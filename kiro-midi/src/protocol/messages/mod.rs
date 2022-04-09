pub mod channel_voice;
pub mod utility;

use crate::protocol::messages::channel_voice::ChannelVoice;
use crate::protocol::messages::utility::Utility;

use self::channel_voice::ChannelVoice1;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Message {
  pub group: u8,
  pub mtype: MessageType,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MessageType {
  Utility(Utility),
  ChannelVoice(ChannelVoice),
  ChannelVoice1(ChannelVoice1),
}
