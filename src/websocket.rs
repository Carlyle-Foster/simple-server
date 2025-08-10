use core::{fmt, str};
use std::{error::Error, io::{self, Write}, mem};

use sha1::{Digest, Sha1};
use base64::prelude::*;

use crate::{helpers::{Parser, SendTo}, http, server_G::{HandshakeStatus, Handshaker, Server_G}, smithy::{self, HttpSmith, HttpSmithText}, StreamId};

pub type WsServer = Server_G<Messenger, WsParser, Message, WebSocketError, WsHandshaker>;

impl WsServer {
    pub fn send_message(&mut self, id: StreamId, message: impl Into<Message>) {
        let m: Message = message.into();
        self.send_to_client(id, m);
    }
}

pub fn compute_sec_websocket_accept(key: &str) -> String {
    let mut sha1 = Sha1::new();
    sha1.update(format!("{key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11"));
    BASE64_STANDARD.encode(sha1.finalize())
}

#[derive(Default, Clone)]
pub struct WsParser {
    incoming_message: Option<Message>,
    skip: usize,
}

impl Parser<Message, WebSocketError> for WsParser {
    fn parse<'b>(&mut self, buf: &'b [u8]) -> Result<(Option<Message>, &'b [u8]), WebSocketError> {
        match parse_message(&mut self.incoming_message, &buf[self.skip..]) {
            Ok((msg, rest)) => {
                self.skip = 0;
                Ok((Some(msg), rest))
            },
            Err(WebSocketError::WOULD_BLOCK) => Ok((None, buf)),
            Err(e) => Err(e),
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct WsHandshaker {
    buf: Vec<u8>,
    bytes_writ: usize,
    parser: HttpSmithText,
}

impl SendTo for WsHandshaker {
    fn send_to(&mut self, wr: &mut impl Write) -> io::Result<usize> {
        let mut slice = (&self.buf[self.bytes_writ..]);
        let bytes = io::copy(&mut slice, wr)? as usize;
        self.bytes_writ += bytes;
        println!("Handshaker: sent {bytes} bytes, bytes_writ = {}", self.bytes_writ);

        Ok(bytes)
    }
}

impl Handshaker for WsHandshaker {
    fn handshake<'b>(&mut self, buf: &'b [u8]) -> Option<(crate::server_G::HandshakeStatus, &'b [u8])> {
        let len = self.buf.len();
        debug_assert!(self.bytes_writ <= len);
        println!("at start, len = {}, bytes_writ = {}", len, self.bytes_writ);
        if len > 0 {
            if self.bytes_writ < len {
                return Some((HandshakeStatus::Responding, buf))
            }
            else {
                return Some((HandshakeStatus::Done, buf))
            }
        }
        match self.parser.deserialize(buf) {
            Ok((handshake, rest)) => {
                let key = handshake.headers.get("sec-websocket-key")?;
                let accept = compute_sec_websocket_accept(key);
                let mut response: http::Response = http::Status::SwitchingProtocols.into();
                response.add_header("Upgrade", "websocket");
                response.add_header("Connection", "Upgrade");
                // TODO: unecessary clone
                response.add_header("Sec-WebSocket-Accept", &accept);
                let (data, _) = self.parser.serialize(&response);
                println!("WEBSOCKET: handshake response length = {}", data.len());
                self.buf = data;
                Some((HandshakeStatus::Responding, rest))
            }
            Err(smithy::ParseError::Incomplete) => Some((HandshakeStatus::Waiting, buf)),
            Err(e) => {
                println!("WEBSOCKET: handshake Error: {e}");
                None
            },
        }
    }
}

fn parse_message<'b>(incoming_message: &mut Option<Message>, mut buf: &'b [u8]) -> Result<(Message, &'b [u8]), WebSocketError> {
    use OPCODE::*;
    use WebSocketError::*;
    loop {
        let (frame, rest) = read_frame(buf)?;
        buf = rest;
        match frame.opcode {
            Continuation => {
                if let Some(ref mut message) = incoming_message {
                    *incoming_message = match message {
                        Message::Text(t) => Message::Text(frame.unmask_into_text(mem::take(t))?),
                        Message::Binary(b) => Message::Binary(frame.unmask_into_binary(mem::take(b))),
                    }.into();
                    if frame.fin { 
                        return Ok((incoming_message.take().unwrap(), rest));
                    }
                    continue
                }
                else { return Err(BAD_CONTINUE) }
            }
            Text => {
                if incoming_message.is_some() { return Err(CUTTING_IN) }
                *incoming_message = Some(Message::Text(frame.unmask_into_text(String::with_capacity(256))?));
                if frame.fin { 
                    return Ok((incoming_message.take().unwrap(), rest)); 
                }
                else { continue }
            },
            Binary => {
                if incoming_message.is_some() { return Err(CUTTING_IN) }
                *incoming_message = Some(Message::Binary(frame.unmask_into_binary(Vec::with_capacity(256))));
                if frame.fin { 
                    return Ok((incoming_message.take().unwrap(), rest));
                }
                else { continue }
            }
            Close => { return Err(UNIMPLEMENTED) },
            Ping => { return Err(UNIMPLEMENTED) },
            Pong => { return Err(UNIMPLEMENTED) },
        }
    }
}
fn read_frame(data: &[u8]) -> Result<(Frame, &[u8]), WebSocketError> {
    use WebSocketError::*;

    let mut mask_offset = 1 + 1;
    const mask_len: usize = 4;
    if data.len() < (mask_offset + mask_len) { return Err(WOULD_BLOCK) }

    let fin = data[0] & 0b1000_0000 > 0;
    if (data[0] & 0b0111_0000) != 0 { return Err(UNRESERVED) }
    let opcode = OPCODE::parse(data[0] & 0b_1111)?;
    if (data[1] & 0b1000_0000) == 0 { return Err(UNMASKED) }
    let payload_len = match (data[1] & 0b0111_1111) {
        l if l < 126 => l as u64,
        126 => {
            mask_offset += 2;
            if data.len() < mask_offset + mask_len { return Err(WOULD_BLOCK) }
            u16::from_be_bytes(data[2..4].try_into().unwrap()) as u64
        },
        127 => {
            mask_offset += 8;
            if data.len() < mask_offset + mask_len { return Err(WOULD_BLOCK) }
            u64::from_be_bytes(data[2..10].try_into().unwrap())
        }
        _ => unreachable!(),
    };
    let mask_key: [u8;4] = data[mask_offset..][..mask_len].try_into().unwrap();
    let payload = &data[mask_offset + mask_len..];
    println!("PAYLOAD_LENGTH = {payload_len}, ACTUAL_LENGTH = {}", payload.len());
    let (payload, rest) = payload.split_at_checked(payload_len as usize).ok_or(WOULD_BLOCK)?;

    let frame = Frame {
        fin,
        opcode,
        mask_key: Some(mask_key),
        payload,
    };

    println!("Frame: fin = {}, opcode = {:?}", frame.fin, frame.opcode);

    Ok((frame, rest))
}

#[derive(Debug)]
#[derive(PartialEq)]
pub enum WebSocketError {
    UNRESERVED,
    UNMASKED,
    BAD_OPCODE,
    BAD_CONTINUE,
    CUTTING_IN,
    NOT_VALID_UTF8,
    CLOSED_BY_CLIENT,
    WOULD_BLOCK,
    WET_HANDSHAKE,
    UNIMPLEMENTED,
}

impl fmt::Display for WebSocketError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl Error for WebSocketError{}

#[derive(Debug, Clone, Copy)]
enum OPCODE {
    Continuation = 0b0,
    Text    = 0b0001,
    Binary  = 0b0010,
    Close   = 0b1000,
    Ping    = 0b1001,
    Pong    = 0b1010,
}

impl OPCODE {
    fn parse(value: u8) -> Result<Self, WebSocketError> {
        use OPCODE::*;
        use WebSocketError::*;

        let code = match value {
            0b0000 => Continuation,
            0b0001 => Text,
            0b0010 => Binary,
            0b1000 => Close,
            0b1001 => Ping,
            0b1010 => Pong,
            _ => return Err(BAD_OPCODE),
        };
        Ok(code)
    }
}

struct Frame<'buf> {
    pub fin: bool,
    pub opcode: OPCODE,
    pub mask_key: Option<[u8;4]>,
    pub payload: &'buf [u8],
}

impl Frame<'_> {
    pub fn unmask_into_binary(&self, mut buffer: Vec<u8>) -> Vec<u8> {
        assert!(buffer.len() <= self.payload.len());

        if let Some(mask_key) = self.mask_key {
            for (index, byte)  in self.payload.iter().enumerate() {
                buffer.push(byte ^ mask_key[index % 4])
            }
        } else {
            buffer.append(&mut self.payload.to_owned());
        }
        buffer
    }
    pub fn unmask_into_text(&self, buffer: String) -> Result<String, WebSocketError> {
        use WebSocketError::*;
        assert!(buffer.len() <= self.payload.len());

        let mut buffer = buffer.into_bytes();
        let initial_length = buffer.len();
        if let Some(mask_key) = self.mask_key {
            for (index, byte)  in self.payload.iter().enumerate() {
                buffer.push(byte ^ mask_key[index % 4])
            }
        } else {
            buffer.append(&mut self.payload.to_owned());
        }
        let _ = str::from_utf8(&buffer[initial_length..]).map_err(|_e| NOT_VALID_UTF8)?;
        return Ok( unsafe {String::from_utf8_unchecked(buffer)} )
    }    
}

#[derive(Debug, Clone)]
pub enum Message {
    Text(String),
    Binary(Vec<u8>),
}

impl Default for Message {
    fn default() -> Self {
        Self::Binary(Vec::default())
    }
}

impl From<Vec<u8>> for Message {
    fn from(value: Vec<u8>) -> Self {
        Self::Binary(value)
    }
}
impl From<String> for Message {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl<'a> From<&'a Message> for Frame<'a> {
    fn from(msg: &'a Message) -> Self {
        match msg {
            Message::Binary(contents) => Frame {
                fin: true,
                opcode: OPCODE::Binary,
                mask_key: None,
                payload: contents,
            },
            Message::Text(contents) => Frame { 
                fin: true, 
                opcode: OPCODE::Text, 
                mask_key: None, 
                payload: contents.as_bytes(),
            }
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct Messenger {
    msg: Message,
    writ: usize,
}

impl From<Message> for Messenger {
    fn from(value: Message) -> Self {
        Self { msg: value, writ: 0 }
    }
}
impl From<Vec<u8>> for Messenger {
    fn from(value: Vec<u8>) -> Self {
        let m: Message  = value.into();
        m.into()
    }
}

impl SendTo for Messenger {
    fn send_to(&mut self, wr: &mut impl Write) -> io::Result<usize> {
        let f: Frame = (&self.msg).into();

        const limit1: usize = 126;
        const limit2: usize = 1 << 16;

        let len = f.payload.len();
        let header_len = match len {
            0..limit1 => 2,
            limit1..limit2 => 2 + 2,
            limit2.. => 2 + 8,
        };
        let bytes = if self.writ < header_len {
            let mut header = [0; 2 + 8];
            header[0] = (1 << 7) | (f.opcode as u8);
            header[1] = match header_len {
                2 => len as u8,
                4 => {
                    header[2..][..2].copy_from_slice(&(len as u16).to_be_bytes());
                    126
                },
                10 => {
                    header[2..][..8].copy_from_slice(&(len as u64).to_be_bytes());
                    127
                },
                _ => unreachable!()
            };
            wr.write(&header[self.writ..header_len])?
        }
        else {
            wr.write(&f.payload[self.writ - header_len..])?
        };

        self.writ += bytes;
        Ok(bytes)
    }
}