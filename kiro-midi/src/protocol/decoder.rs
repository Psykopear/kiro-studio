use thiserror::Error;

use crate::filter::Filter;
use crate::messages::channel_voice::ChannelVoice1;
use crate::protocol::messages::channel_voice::ChannelVoice;
use crate::protocol::messages::utility::Utility;
use crate::protocol::messages::{Message, MessageType};
use crate::protocol::Decode;

#[derive(Debug, Error)]
pub enum Error {
  #[error("Found reserved encoding")]
  Reserved,
}

pub trait DecoderProtocol {
  fn get_index(&self) -> usize;
  fn set_index(&mut self, index: usize);
  fn decode(&mut self, mtype: u8, group: u8, filter: &Filter) -> Option<Message>;
  fn get_len(&self) -> usize;
  fn set_len(&mut self, len: usize);
  fn get_ump_mut(&mut self) -> &mut [u32; 4];
  fn get_ump(&self) -> &[u32; 4];

  fn increase_index(&mut self) {
    let index = self.get_index();
    self.set_index(index + 1);
  }

  fn next(&mut self, data: u32, filter: &Filter) -> Result<Option<Message>, Error> {
    if self.get_index() == 0 {
      self.init(data);
    }
    self.push(data);
    println!("Len: {}", self.get_len());
    println!("Iscomplete: {}", self.is_complete());

    let next_message = if self.is_complete() {
      let (mtype, group) = self.extract_mtype_and_group();
      println!("mtype: {}, group: {}", mtype, group);
      let message = if filter.mtype(mtype) && filter.group(group) {
        self.decode(mtype, group, filter)
      } else {
        None
      };
      println!("Message: {:?}", message);
      self.reset();
      message
    } else {
      None
    };

    Ok(next_message)
  }

  fn init(&mut self, data: u32) {
    let mtype = (data >> 28) & 0x0f;
    self.set_len(match mtype {
      0x00 => 1,
      0x01 => 1,
      0x02 => 1,
      0x03 => 2,
      0x04 => 2,
      0x05 => 4,
      _ => 1,
    });
  }

  fn push(&mut self, data: u32) {
    let index = self.get_index();
    self.get_ump_mut()[index] = data;
    self.increase_index();
  }

  fn is_complete(&self) -> bool {
    self.get_index() == self.get_len()
  }

  fn extract_mtype_and_group(&self) -> (u8, u8) {
    let mtype = ((self.get_ump()[0] >> 28) & 0x0f) as u8;
    let group = ((self.get_ump()[0] >> 24) & 0x0f) as u8;
    (mtype, group)
  }

  fn reset(&mut self) {
    self.set_index(0);
    self.set_len(0);
  }
}

#[derive(Default)]
pub struct DecoderProtocol1 {
  ump: [u32; 4],
  index: usize,
  len: usize,
}

impl DecoderProtocol for DecoderProtocol1 {
  fn decode(&mut self, mtype: u8, group: u8, filter: &Filter) -> Option<Message> {
    match mtype {
      0x02 => {
        let channel_voice = ChannelVoice1::decode(&self.ump[0..1]);
        filter
          .channel(group, channel_voice.channel)
          .then(|| Message {
            group,
            mtype: MessageType::ChannelVoice1(channel_voice),
          })
      }
      _ => None,
    }
  }

  fn get_index(&self) -> usize {
    self.index
  }

  fn set_index(&mut self, index: usize) {
    self.index = index;
  }

  fn get_len(&self) -> usize {
    self.len
  }

  fn set_len(&mut self, len: usize) {
    self.len = len;
  }

  fn get_ump_mut(&mut self) -> &mut [u32; 4] {
    &mut self.ump
  }

  fn get_ump(&self) -> &[u32; 4] {
    &self.ump
  }
}

#[derive(Default)]
pub struct DecoderProtocol2 {
  ump: [u32; 4],
  index: usize,
  len: usize,
}

impl DecoderProtocol for DecoderProtocol2 {
  fn decode(&mut self, mtype: u8, group: u8, filter: &Filter) -> Option<Message> {
    match mtype {
      0x00 => Some(Message {
        group,
        mtype: MessageType::Utility(Utility::decode(&self.ump[0..1])),
      }),
      0x04 => {
        let channel_voice = ChannelVoice::decode(&self.ump[0..2]);
        filter
          .channel(group, channel_voice.channel)
          .then(|| Message {
            group,
            mtype: MessageType::ChannelVoice(channel_voice),
          })
      }
      _ => None,
    }
  }
  fn get_index(&self) -> usize {
    self.index
  }

  fn set_index(&mut self, index: usize) {
    self.index = index;
  }

  fn get_len(&self) -> usize {
    self.len
  }

  fn set_len(&mut self, len: usize) {
    self.len = len;
  }

  fn get_ump_mut(&mut self) -> &mut [u32; 4] {
    &mut self.ump
  }

  fn get_ump(&self) -> &[u32; 4] {
    &self.ump
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::protocol::decoder::{DecoderProtocol2, DecoderProtocol};
  use crate::protocol::messages::channel_voice::ChanelVoiceMessage;

  #[test]
  fn first_word_does_not_emit() {
    let filter = Filter::new();
    let mut decoder = DecoderProtocol2::default();

    let result = decoder.next(0x40903c00, &filter);

    assert!(
      matches!(result, Ok(None)),
      "Unexpected result: {:?}",
      result
    );
  }

  #[test]
  fn last_word_emits() {
    let filter = Filter::new();
    let mut decoder = DecoderProtocol2::default();

    decoder.next(0x41923c00, &filter);
    let result = decoder.next(0xabcd0000, &filter);
    assert!(
      matches!(&result, Ok(Some(message)) if message == &Message {
        group: 1,
        mtype: MessageType::ChannelVoice(ChannelVoice {
          channel: 2,
          message: ChanelVoiceMessage::NoteOn {
            note: 0x3c,
            velocity: 0xabcd,
            attr_type: 0,
            attr_data: 0,
          }
        })
      }),
      "Unexpected result: {:?}",
      result
    );
  }

  #[test]
  fn two_messages_are_emitted() {
    let filter = Filter::new();
    let mut decoder = DecoderProtocol2::default();

    decoder.next(0x41923c00, &filter);
    let result = decoder.next(0xabcd0000, &filter);
    assert!(
      matches!(&result, Ok(Some(_))),
      "Unexpected result: {:?}",
      result
    );
    decoder.next(0x43853d00, &filter);
    let result = decoder.next(0x12340000, &filter);
    assert!(
      matches!(&result, Ok(Some(message)) if message == &Message {
        group: 3,
        mtype: MessageType::ChannelVoice(ChannelVoice {
          channel: 5,
          message: ChanelVoiceMessage::NoteOff {
            note: 0x3d,
            velocity: 0x1234,
            attr_type: 0,
            attr_data: 0,
          }
        })
      }),
      "Unexpected result: {:?}",
      result
    );
  }
}
