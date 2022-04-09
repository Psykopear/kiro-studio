use arc_swap::ArcSwap;
use jack::{AsyncClient, Client, MidiIn, NotificationHandler, Port, ProcessHandler, Unowned};
use std::{
  collections::{HashMap, HashSet},
  sync::{Arc, Mutex},
};
use thiserror::Error;

use crate::{
  drivers,
  endpoints::{DestinationInfo, Endpoints, SourceId, SourceInfo},
  protocol::decoder::{DecoderProtocol, DecoderProtocol1},
  Event, Filter, InputConfig, InputHandler, InputInfo, SourceMatches,
};

#[derive(Error, Debug)]
pub enum JackMidiError {
  #[error("Error creating a jack client")]
  ClientCreate,
  #[error("Error creating a jack port")]
  PortCreate,
  #[error("An input with this name already exists: {0:?}")]
  InputAlreadyExists(InputConfig),
  #[error("Input not found: {0}")]
  InputNotFound(InputName),
}

type InputName = String;
struct Input {
  name: InputName,
  sources: SourceMatches,
  connected: HashSet<SourceId>,
  filters: Arc<ArcSwap<HashMap<SourceId, Filter>>>,
  port: Port<MidiIn>,
  handler: InputHandler,
  decoder: DecoderProtocol1,
}

#[derive(Clone)]
struct JackHost {
  pub endpoints: Arc<Mutex<Endpoints<Port<Unowned>, Port<Unowned>>>>,
  pub inputs: Arc<Mutex<HashMap<String, Input>>>,
}

struct Notifications {
  pub endpoints: Arc<Mutex<Endpoints<Port<Unowned>, Port<Unowned>>>>,
  pub inputs: Arc<Mutex<HashMap<String, Input>>>,
}

pub struct JackMidiDriver {
  active_client: AsyncClient<Notifications, JackHost>,
  host: Arc<JackHost>,
}

impl NotificationHandler for Notifications {
  fn thread_init(&self, _: &Client) {
    println!("Thread init");
  }

  fn shutdown(&mut self, _status: jack::ClientStatus, _reason: &str) {
    println!("Shutdown");
  }

  fn freewheel(&mut self, _: &Client, _is_freewheel_enabled: bool) {
    println!("Freewheel");
  }

  fn sample_rate(&mut self, _: &Client, srate: jack::Frames) -> jack::Control {
    println!("Sample rate {srate}");
    jack::Control::Continue
  }

  fn client_registration(&mut self, _: &Client, _name: &str, _is_registered: bool) {
    println!("Client registration");
  }

  fn port_registration(&mut self, _: &Client, _port_id: jack::PortId, _is_registered: bool) {
    println!("Port registration");
  }

  fn port_rename(
    &mut self,
    _: &Client,
    _port_id: jack::PortId,
    _old_name: &str,
    _new_name: &str,
  ) -> jack::Control {
    // println!("Port rename");
    jack::Control::Continue
  }

  fn ports_connected(
    &mut self,
    client: &Client,
    port_id_a: jack::PortId,
    port_id_b: jack::PortId,
    are_connected: bool,
  ) {
    // TODO: Handle disconnection
    if !are_connected {
      println!("Port disconnected");
      return;
    }
    let source_id = port_id_a as u64;
    let name = client.port_by_id(port_id_a).unwrap().name().unwrap();
    let port = client.port_by_id(port_id_b).unwrap();
    let mut endpoints = self.endpoints.lock().unwrap();
    endpoints.add_source(source_id, name.clone(), port.clone());
    for input in self.inputs.lock().unwrap().values_mut() {
      if !input.connected.contains(&source_id) {
        if let Some(filter) = input.sources.match_filter(source_id, name.as_str()) {
          let mut filters = input.filters.load().as_ref().clone();
          filters.insert(source_id, filter);
          input.filters.swap(Arc::new(filters));
          input.connected.insert(source_id);
        }
      }
    }
    println!("Port connected");
  }

  fn graph_reorder(&mut self, _: &Client) -> jack::Control {
    // println!("Graph reorder");
    jack::Control::Continue
  }

  fn xrun(&mut self, _: &Client) -> jack::Control {
    println!("XRUN!");
    jack::Control::Continue
  }
}

impl ProcessHandler for JackHost {
  fn process(&mut self, _: &jack::Client, ps: &jack::ProcessScope) -> jack::Control {
    for input in self.inputs.lock().unwrap().values_mut() {
      for source_id in input.connected.iter() {
        let default_filter = Filter::new();
        let filters = input.filters.load();
        let filter = filters.get(&source_id).unwrap_or(&default_filter);
        let show_p = input.port.iter(ps);
        input.decoder.reset();
        for word in show_p {
          // The first byte indicates the rest of the message is MIDI1
          let bytes: [u8; 4] = match word.bytes {
            [one, two, three] => [0b0010_0000, *one, *two, *three],
            [one, two] => [0b0010_0000, *one, *two, 0],
            _ => panic!(),
          };
          let bytes = u32::from_be_bytes(bytes);
          if let Ok(Some(message)) = input.decoder.next(bytes, &filter) {
            let event = Event {
              timestamp: word.time as u64,
              endpoint: *source_id,
              message,
            };
            input.handler.call(event);
          }
        }
      }
    }
    jack::Control::Continue
  }
}

impl JackMidiDriver {
  pub fn new(name: &str) -> Result<Self, drivers::Error> {
    let endpoints = Arc::new(Mutex::new(Endpoints::new()));
    let inputs = Arc::new(Mutex::new(HashMap::new()));
    let mut host = Arc::new(JackHost { endpoints, inputs });
    let not_host = Arc::make_mut(&mut host);
    let (client, _status) = jack::Client::new(name, jack::ClientOptions::NO_START_SERVER)
      .map_err(|_| JackMidiError::ClientCreate)?;
    let active_client = client
      .activate_async(
        Notifications {
          inputs: not_host.inputs.clone(),
          endpoints: not_host.endpoints.clone(),
        },
        not_host.to_owned(),
      )
      .unwrap();
    Ok(Self {
      host,
      active_client,
    })
  }
}

impl drivers::DriverSpec for JackMidiDriver {
  fn create_input<H>(
    &mut self,
    config: crate::InputConfig,
    handler: H,
  ) -> Result<String, drivers::Error>
  where
    H: Into<crate::InputHandler>,
  {
    let host = &self.host;
    if host
      .inputs
      .lock()
      .map_err(|_| JackMidiError::PortCreate)?
      .contains_key(config.name.as_str())
    {
      return Err(JackMidiError::InputAlreadyExists(config).into());
    };

    let InputConfig { name, sources } = config;
    let client = self.active_client.as_client();
    let endpoints = host.endpoints.lock().unwrap();
    let filters = endpoints
      .connected_sources()
      .into_iter()
      .filter_map(|connected_source| {
        sources
          .match_filter(connected_source.id, connected_source.name.as_str())
          .map(|filter| (connected_source.id, filter))
      })
      .collect::<HashMap<SourceId, Filter>>();

    let filters = Arc::new(ArcSwap::new(Arc::new(filters)));
    let port = client
      .register_port(&name, MidiIn)
      .map_err(|_| JackMidiError::PortCreate)?;

    let connected: HashSet<u64> = filters
      .load()
      .keys()
      .into_iter()
      .filter_map(|source_id| {
        endpoints.get_source(*source_id).and_then(|source| {
          client
            .connect_ports(&source, &port)
            .map_or_else(|_err| None, |_| Some(*source_id))
        })
      })
      .collect();

    let input = Input {
      name: name.clone(),
      sources,
      connected,
      filters,
      port,
      handler: handler.into(),
      decoder: DecoderProtocol1::default(),
    };
    host.inputs.lock().unwrap().insert(name.clone(), input);
    Ok(name)
  }

  fn sources(&self) -> Vec<crate::endpoints::SourceInfo> {
    let inputs = self.host.inputs.lock().unwrap();
    let mut source_inputs: HashMap<SourceId, Vec<String>> = inputs
      .values()
      .fold(
        HashMap::new(),
        |mut map: HashMap<SourceId, HashSet<String>>, input| {
          for source_id in input.connected.iter() {
            map
              .entry(*source_id)
              .or_default()
              .insert(input.name.clone());
          }
          map
        },
      )
      .into_iter()
      .map(|(id, value)| (id, value.into_iter().collect::<Vec<String>>()))
      .collect();

    let endpoints = self.host.endpoints.lock().unwrap();
    endpoints
      .connected_sources()
      .into_iter()
      .map(|connected_source| {
        SourceInfo::new(
          connected_source.id,
          connected_source.name.clone(),
          source_inputs
            .remove(&connected_source.id)
            .unwrap_or_default(),
        )
      })
      .collect()
  }

  fn destinations(&self) -> Vec<crate::endpoints::DestinationInfo> {
    self
      .host
      .endpoints
      .lock()
      .unwrap()
      .connected_destinations()
      .into_iter()
      .map(|connected_destination| {
        DestinationInfo::new(connected_destination.id, connected_destination.name.clone())
      })
      .collect()
  }

  fn inputs(&self) -> Vec<crate::InputInfo> {
    self
      .host
      .inputs
      .lock()
      .unwrap()
      .values()
      .map(|input| InputInfo {
        name: input.name.clone(),
        sources: input.sources.clone(),
        connected_sources: input.connected.iter().cloned().collect(),
      })
      .collect()
  }

  fn get_input_config(&self, name: &str) -> Option<crate::InputConfig> {
    self
      .host
      .inputs
      .lock()
      .unwrap()
      .get(name)
      .map(|input| InputConfig {
        name: input.name.clone(),
        sources: input.sources.clone(),
      })
  }

  fn set_input_sources(
    &self,
    name: &str,
    sources: crate::SourceMatches,
  ) -> Result<(), drivers::Error> {
    let host = &self.host;
    let endpoints = host.endpoints.lock().unwrap();

    let mut inputs = host.inputs.lock().unwrap();

    let input = inputs
      .get_mut(name)
      .ok_or_else(|| JackMidiError::InputNotFound(name.to_string()))?;

    let connected_sources = endpoints
      .connected_sources()
      .into_iter()
      .filter_map(|connected_source| {
        sources
          .match_filter(connected_source.id, connected_source.name.as_str())
          .map(|filter| (connected_source.id, filter, &connected_source.source))
      })
      .collect::<Vec<(SourceId, Filter, &Port<Unowned>)>>();

    let mut filters = HashMap::<SourceId, Filter>::with_capacity(connected_sources.len());
    let mut disconnected = input.connected.clone();

    let client = self.active_client.as_client();
    for (source_id, filter, source) in connected_sources {
      filters.insert(source_id, filter);
      if !input.connected.contains(&source_id) {
        if let Ok(()) = client.connect_ports(&source, &input.port) {
          input.connected.insert(source_id);
        }
      } else {
        disconnected.remove(&source_id);
      }
    }

    for source_id in disconnected {
      if let Some(source) = endpoints.get_source(source_id) {
        client.disconnect_ports(source, &input.port).ok();
      }
    }

    input.sources = sources;
    input.filters.swap(Arc::new(filters));

    Ok(())
  }
}
