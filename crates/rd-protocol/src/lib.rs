//! RDX 协议层：数据面裸帧格式 +（P2 起）控制面 protobuf 消息。
//!
//! 见 ADR-0003：媒体帧绝不进入 protobuf，控制与数据分离。

pub mod frame;

pub use frame::{flags, Frame, FrameError, FrameHeader, HEADER_LEN, MAX_PAYLOAD_LEN};
