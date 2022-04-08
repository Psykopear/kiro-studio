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
  messages::{utility::Utility, Message, MessageType},
  Event, Filter, InputConfig, InputHandler, InputInfo, SourceMatches,
};

const MAX_MIDI: usize = 3;

#[derive(Copy, Clone)]
struct MidiCopy {
  len: usize,
  data: [u8; MAX_MIDI],
  time: jack::Frames,
}

impl From<jack::RawMidi<'_>> for MidiCopy {
  fn from(midi: jack::RawMidi<'_>) -> Self {
    let len = std::cmp::min(MAX_MIDI, midi.bytes.len());
    let mut data = [0; MAX_MIDI];
    data[..len].copy_from_slice(&midi.bytes[..len]);
    MidiCopy {
      len,
      data,
      time: midi.time,
    }
  }
}

impl std::fmt::Debug for MidiCopy {
  fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
    write!(
      f,
      "Midi {{ time: {}, len: {}, data: {:?} }}",
      self.time,
      self.len,
      &self.data[..self.len]
    )
  }
}

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

  fn sample_rate(&mut self, _: &Client, _srate: jack::Frames) -> jack::Control {
    println!("Sample rate");
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
    println!("Port rename");
    jack::Control::Continue
  }

  fn ports_connected(
    &mut self,
    client: &Client,
    _port_id_a: jack::PortId,
    _port_id_b: jack::PortId,
    _are_connected: bool,
  ) {
    let source_id = _port_id_a as u64;
    let name = client.port_by_id(_port_id_a).unwrap().name().unwrap();
    let port = client.port_by_id(_port_id_b).unwrap();
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
    println!("Graph reorder");
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
      let show_p = input.port.iter(ps);
      for e in show_p {
        // TODO: Build real message
        let c: Event = Event {
          endpoint: 0,
          message: Message {
            group: 0,
            mtype: MessageType::Utility(Utility::Noop),
          },
          timestamp: e.time as u64,
        };
        input.handler.call(c);
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
      // client: Some(client),
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
    // dbg!(&config.name);
    if host
      .inputs
      .lock()
      .map_err(|_| JackMidiError::PortCreate)?
      .contains_key(config.name.as_str())
    {
      return Err(JackMidiError::InputAlreadyExists(config).into());
    };
    println!("Input 1");
    let InputConfig { name, sources } = config;
    let client = self.active_client.as_client();
    let filters = host
      .endpoints
      .lock()
      .unwrap()
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

    let mut connected = HashSet::new();
    let host = &self.host;
    let endpoints = host.endpoints.lock().unwrap();
    for source_id in filters.load().keys().cloned() {
      if let Some(source) = endpoints.get_source(source_id) {
        if let Ok(()) = client.connect_ports(&source, &port) {
          connected.insert(source_id);
        }
      }
    }
    let input = Input {
      name: name.clone(),
      sources,
      connected,
      filters,
      port,
      handler: handler.into(),
    };
    host.inputs.lock().unwrap().insert(name.clone(), input);
    Ok(name)
  }

  fn sources(&self) -> Vec<crate::endpoints::SourceInfo> {
    let host = &self.host;
    let endpoints = host.endpoints.lock().unwrap();

    let mut source_inputs = HashMap::<SourceId, HashSet<String>>::new();
    for input in host.inputs.lock().unwrap().values() {
      for source_id in input.connected.iter().cloned() {
        let inputs = source_inputs.entry(source_id).or_default();
        inputs.insert(input.name.clone());
      }
    }
    // dbg!(&source_inputs);

    endpoints
      .connected_sources()
      .into_iter()
      .map(|connected_source| {
        // dbg!(&connected_source);
        let inputs = source_inputs
          .get(&connected_source.id)
          .map(|inputs| inputs.iter().cloned().collect::<Vec<String>>())
          .unwrap_or_default();
        SourceInfo::new(connected_source.id, connected_source.name.clone(), inputs)
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
        // dbg!(&connected_destination);
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
