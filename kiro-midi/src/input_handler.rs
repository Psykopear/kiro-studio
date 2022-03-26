use ringbuf::Producer;
use std::fmt::{Debug, Formatter};

use crate::event::MidiEvent;

pub enum InputHandler {
  Callback(Box<dyn FnMut(MidiEvent) + Send + 'static>),
  RingBuffer(Producer<MidiEvent>),
}

impl InputHandler {
  pub(crate) fn call(&mut self, event: MidiEvent) {
    match self {
      InputHandler::Callback(ref mut callback) => (callback)(event),
      InputHandler::RingBuffer(ref mut producer) => {
        producer.push(event).ok();
      }
    };
  }
}

impl<F> From<F> for InputHandler
where
  F: FnMut(MidiEvent) + Send + 'static,
{
  fn from(callback: F) -> Self {
    InputHandler::Callback(Box::new(callback))
  }
}

impl From<Producer<MidiEvent>> for InputHandler {
  fn from(producer: Producer<MidiEvent>) -> Self {
    InputHandler::RingBuffer(producer)
  }
}

impl Debug for InputHandler {
  fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
    match self {
      Self::Callback(_) => write!(f, "Callback"),
      Self::RingBuffer(_) => write!(f, "RingBuffer"),
    }
  }
}

#[cfg(test)]
mod tests {
  use crate::protocol::messages::utility::Utility;
  use crate::protocol::messages::MessageType;
  use std::sync::atomic::{AtomicU8, Ordering};
  use std::sync::Arc;

  use super::*;

  #[test]
  fn from_callback() {
    let state = Arc::new(AtomicU8::new(1));
    let state_clone = state.clone();

    let mut handler = InputHandler::from(move |event: MidiEvent| {
      state_clone.store(event.message.group, Ordering::Relaxed)
    });

    handler.call(MidiEvent {
      timestamp: 0,
      endpoint: 0,
      message: Message {
        group: 8,
        mtype: MessageType::Utility(Utility::Noop),
      },
    });

    assert_eq!(state.load(Ordering::Relaxed), 8);
  }

  #[test]
  fn from_ring_buffer() {
    let (mut producer, mut consumer) = ringbuf::RingBuffer::new(1).split();
    let event = MidiEvent {
      timestamp: 0,
      endpoint: 0,
      message: Message {
        group: 8,
        mtype: MessageType::Utility(Utility::Noop),
      },
    };

    let mut handler = InputHandler::from(producer);

    handler.call(event.clone());

    assert_eq!(consumer.pop(), Some(event));
  }
}
