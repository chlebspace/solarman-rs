use std::io;

use tokio_util::{
    bytes::{Buf, BytesMut},
    codec::{Decoder, Encoder},
};

use crate::{PacketEncode, ParsedPacket, parse};

pub struct SolarmanCodec;

impl<T> Encoder<T> for SolarmanCodec
where
    T: PacketEncode,
{
    type Error = io::Error;

    fn encode(
        &mut self,
        item: T,
        dst: &mut tokio_util::bytes::BytesMut,
    ) -> Result<(), Self::Error> {
        let init_len = dst.len();
        let frame_size = item.size();
        dst.resize(init_len + frame_size, 0);
        item.encode(&mut dst[init_len..]);
        Ok(())
    }
}

impl Decoder for SolarmanCodec {
    type Item = ParsedPacket;
    type Error = crate::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let Some((packet, frame_size)) = parse(src)? else {
            return Ok(None);
        };
        src.advance(frame_size);
        Ok(Some(packet))
    }
}
