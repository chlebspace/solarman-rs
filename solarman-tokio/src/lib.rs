use std::{time::Duration, u8};

use futures::{SinkExt, StreamExt};
use solarman_protocol::{ParsedPacket, SolarmanCodec};
use thiserror::Error;
use tokio::{
    io::AsyncWriteExt,
    net::{TcpStream, ToSocketAddrs},
};
use tokio_util::codec::Framed;

/// Error type used by the library.
#[derive(Error, Debug)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("solarman protocol error: {0}")]
    Solarman(#[from] solarman_protocol::Error),
    #[error("modbus request encoding error: {0}")]
    ModbusRequest(#[from] modbus_rtu::error::RequestPacketError),
    #[error("modbus response parsing error: {0}")]
    ModbusResponse(#[from] modbus_rtu::error::ResponsePacketError),
    #[error("modbus exception: {0}")]
    ModbusException(modbus_rtu::Exception),
    #[error("the stream ended unexpectedly")]
    UnexpectedEof,
    #[error("unexpected modbus response received")]
    UnexpectedResponse,
    #[error("received wrong response sequence number")]
    BadSequenceNumber,
    #[error("response doesn't match request serial (wrong serial number)")]
    BadSerial,
}

pub type Result<T> = std::result::Result<T, Error>;

pub struct Client {
    seq: u8,
    remote_seq: Option<u8>,
    serial: u32,
    modbus_id: u8,
    stream: Framed<TcpStream, SolarmanCodec>,
}

impl Client {
    pub async fn connect<A: ToSocketAddrs>(
        addr: A,
        serial: u32,
        modbus_slave_id: u8,
    ) -> Result<Self> {
        let tcp = TcpStream::connect(addr).await?;
        Ok(Self {
            seq: u8::MAX,
            remote_seq: None,
            serial,
            modbus_id: modbus_slave_id,
            stream: Framed::new(tcp, SolarmanCodec),
        })
    }

    async fn execute_modbus(
        &mut self,
        function: &modbus_rtu::Function,
    ) -> Result<modbus_rtu::Response> {
        // TODO: implement timeout
        let modbus_req = modbus_rtu::Request::new(self.modbus_id, function, Duration::ZERO);
        self.seq = self.seq.wrapping_add(1);
        let solarman_req = solarman_protocol::RequestPacket {
            id: self.seq,
            seq: 0,
            serial: self.serial,
            modbus_payload: modbus_req.to_bytes()?,
        };
        tracing::debug!("sending solarman request: {solarman_req:?}");
        self.stream.send(solarman_req).await?;
        let solarman_resp = self.stream.next().await.ok_or(Error::UnexpectedEof)??;
        tracing::debug!("received solarman response: {solarman_resp:?}");
        loop {
            match solarman_resp {
                ParsedPacket::Response(resp) => {
                    if resp.id != self.seq {
                        return Err(Error::BadSequenceNumber);
                    }
                    if resp.serial != self.serial {
                        return Err(Error::BadSerial);
                    }
                    let modbus_resp =
                        modbus_rtu::Response::from_bytes(&modbus_req, &resp.modbus_payload)?;
                    tracing::debug!("modbus response decoded: {modbus_resp:?}");
                    return Ok(modbus_resp);
                }
                ParsedPacket::Unknown(ref packet) => {
                    tracing::warn!("unknown solarman packet received: {packet:?}, skipping");
                }
            }
        }
    }

    pub async fn read_holding_registers(&mut self, addr: u16, quantity: u16) -> Result<Box<[u16]>> {
        match self
            .execute_modbus(&modbus_rtu::Function::ReadHoldingRegisters {
                starting_address: addr,
                quantity,
            })
            .await?
        {
            modbus_rtu::Response::Value(items) => Ok(items),
            modbus_rtu::Response::Exception(exception) => Err(Error::ModbusException(exception)),
            _ => Err(Error::UnexpectedResponse),
        }
    }

    pub async fn shutdown(self) -> Result<()> {
        self.stream
            .into_inner()
            .shutdown()
            .await
            .map_err(Into::into)
    }
}
