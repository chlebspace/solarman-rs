use std::io;

use tokio_util::{
    bytes::{Buf, BytesMut},
    codec::{Decoder, Encoder},
};

use crate::{Frame, PacketEncode, ParsedFrame, parse_frame};

pub struct SolarmanCodec;

impl<P> Encoder<Frame<P>> for SolarmanCodec
where
    P: PacketEncode,
{
    type Error = io::Error;

    fn encode(
        &mut self,
        size: Frame<P>,
        dst: &mut tokio_util::bytes::BytesMut,
    ) -> Result<(), Self::Error> {
        let init_len = dst.len();
        let frame_sz = size.size();
        dst.resize(init_len + frame_sz, 0);
        size.encode(&mut dst[init_len..]);
        Ok(())
    }
}

impl Decoder for SolarmanCodec {
    type Item = ParsedFrame;
    type Error = crate::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let Some((packet, frame_size)) = parse_frame(src)? else {
            return Ok(None);
        };
        src.advance(frame_size);
        Ok(Some(packet))
    }
}
