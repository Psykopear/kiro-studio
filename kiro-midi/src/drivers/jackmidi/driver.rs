use std::{
  collections::{hash_map::DefaultHasher, HashMap, HashSet},
  hash::{Hash, Hasher},
  sync::{Arc, Mutex},
};

use arc_swap::ArcSwap;
use jack::{Client, MidiIn, MidiOut, Port, PortFlags, ProcessHandler};
use thiserror::Error;

use crate::{
  drivers,
  endpoints::{DestinationInfo, SourceId, SourceInfo},
  Filter, InputConfig, InputHandler, InputInfo, SourceMatches,
};

use super::endpoints::Endpoints;

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

pub struct JackMidiHost {
  inputs: HashMap<String, Port<MidiIn>>,
  outputs: Vec<Port<MidiOut>>,
}

impl JackMidiHost {
  pub fn new(client: &jack::Client) -> Self {
    Self {
      inputs: HashMap::new(),
      outputs: vec![],
    }
  }
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

pub struct JackMidiDriver {
  client: Client,
  endpoints: Arc<Mutex<Endpoints>>,
  inputs: Arc<Mutex<HashMap<String, Input>>>,
}

impl ProcessHandler for JackMidiDriver {
  fn process(&mut self, _: &jack::Client, ps: &jack::ProcessScope) -> jack::Control {
    // for input in self.inputs.values() {
    //   let show_p = input.iter(ps);
    //   for e in show_p {
    //     let c: MidiCopy = e.into();
    //     dbg!(c);
    //   }
    // }
    jack::Control::Continue
  }
}

impl JackMidiDriver {
  pub fn new(name: &str) -> Result<Self, drivers::Error> {
    let endpoints = Arc::new(Mutex::new(Endpoints::new()));
    let inputs = Arc::new(Mutex::new(HashMap::new()));
    let (client, _status) = jack::Client::new(name, jack::ClientOptions::NO_START_SERVER)
      .map_err(|_| JackMidiError::ClientCreate)?;
    Ok(Self {
      client,
      endpoints,
      inputs,
    })
  }
}

fn calculate_hash<T>(t: &T) -> u64
where
  T: Hash,
{
  let mut s = DefaultHasher::new();
  t.hash(&mut s);
  s.finish()
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
    if self
      .inputs
      .lock()
      .map_err(|_| JackMidiError::PortCreate)?
      .contains_key(config.name.as_str())
    {
      return Err(JackMidiError::InputAlreadyExists(config).into());
    };
    let InputConfig { name, sources } = config;
    let filters = self
      .client
      .ports(None, None, PortFlags::IS_INPUT)
      .iter()
      .filter_map(|port_name| {
        let id = calculate_hash(port_name);
        sources
          .match_filter(id, port_name.as_str())
          .map(|filter| (id, filter))
      })
      .collect::<HashMap<SourceId, Filter>>();

    let filters = Arc::new(ArcSwap::new(Arc::new(filters)));

    // let mut port = self.client.create_input_port(name.clone(), handler.into(), filters.clone())?;
    let port = self
      .client
      .register_port(&name, MidiIn)
      .map_err(|_| JackMidiError::PortCreate)?;

    let mut connected = HashSet::new();
    let endpoints = self.endpoints.lock().unwrap();
    for source_id in filters.load().keys().cloned() {
      if let Some(source) = endpoints.get_source(source_id) {
        if let Ok(()) = self.client.connect_ports(&source, &port) {
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
    self.inputs.lock().unwrap().insert(name.clone(), input);
    Ok(name)
  }

  fn sources(&self) -> Vec<crate::endpoints::SourceInfo> {
    let endpoints = self.endpoints.lock().unwrap();

    let mut source_inputs = HashMap::<SourceId, HashSet<String>>::new();
    for input in self.inputs.lock().unwrap().values() {
      for source_id in input.connected.iter().cloned() {
        let inputs = source_inputs.entry(source_id).or_default();
        inputs.insert(input.name.clone());
      }
    }

    endpoints
      .connected_sources()
      .into_iter()
      .map(|connected_source| {
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
    let endpoints = self.endpoints.lock().unwrap();

    let mut inputs = self.inputs.lock().unwrap();

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
      .collect::<Vec<(SourceId, Filter, &Port<MidiIn>)>>();

    let mut filters = HashMap::<SourceId, Filter>::with_capacity(connected_sources.len());
    let mut disconnected = input.connected.clone();

    for (source_id, filter, source) in connected_sources {
      filters.insert(source_id, filter);
      if !input.connected.contains(&source_id) {
        if let Ok(()) = self.client.connect_ports(&source, &input.port) {
          input.connected.insert(source_id);
        }
      } else {
        disconnected.remove(&source_id);
      }
    }

    for source_id in disconnected {
      if let Some(source) = endpoints.get_source(source_id) {
        self.client.disconnect_ports(source, &input.port).ok();
      }
    }

    input.sources = sources;
    input.filters.swap(Arc::new(filters));

    Ok(())
  }
}
