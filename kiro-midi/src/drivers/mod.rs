#[cfg(target_os = "macos")]
mod coremidi;
#[cfg(target_os = "macos")]
use crate::drivers::coremidi::{CoreMidiDriver, CoreMidiError};

#[cfg(target_os = "linux")]
mod jackmidi;
#[cfg(target_os = "linux")]
use crate::drivers::jackmidi::{JackMidiDriver, JackMidiError};

use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
  #[cfg(target_os = "macos")]
  #[error("CoreMidi: {0}")]
  CoreMidi(#[from] CoreMidiError),
  #[cfg(target_os = "linux")]
  #[error("Jack: {0}")]
  JackMidi(#[from] JackMidiError),
}

use enum_dispatch::enum_dispatch;

use crate::endpoints::{DestinationInfo, SourceInfo};
use crate::{InputConfig, InputHandler, InputInfo, SourceMatches};

#[enum_dispatch(Driver)]
pub trait DriverSpec {
  fn create_input<H>(&mut self, config: InputConfig, handler: H) -> Result<String, Error>
  where
    H: Into<InputHandler>;
  fn sources(&self) -> Vec<SourceInfo>;
  fn destinations(&self) -> Vec<DestinationInfo>;
  fn inputs(&self) -> Vec<InputInfo>;
  fn get_input_config(&self, name: &str) -> Option<InputConfig>;
  fn set_input_sources(&self, name: &str, sources: SourceMatches) -> Result<(), Error>;
fn activate(&mut self, client: jack::Client);
}

#[enum_dispatch]
pub enum Driver {
  #[cfg(target_os = "macos")]
  CoreMidiDriver,
  #[cfg(target_os = "linux")]
  JackMidiDriver,
}

#[cfg(target_os = "macos")]
pub fn create(name: &str) -> Result<Driver, Error> {
  CoreMidiDriver::new(name).map(Into::into)
}

#[cfg(target_os = "linux")]
pub fn create(name: &str) -> Result<Driver, Error> {
  JackMidiDriver::new(name).map(Into::into)
}
