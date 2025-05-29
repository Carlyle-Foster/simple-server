use core::fmt;
use std::{collections::HashMap, io::{self, ErrorKind, Write}, marker::PhantomData, net::SocketAddr, sync::Arc, time::{Duration, Instant}};

use mio::{net::{TcpListener, TcpStream}, Events, Interest, Poll, Token};
use rustls::ServerConfig;

use crate::{helpers::{throw_reader_at_writer, Parser, SendTo}, Buffer, SERVER, TLS::TLStream, TLS2::StreamId};

pub struct Server_G<M, P, T, E> 
where 
    M: SendTo + Default,
    P: for<'b> Parser<'b, T, E> + Default,
    E: fmt::Display,
{
    pub clients: HashMap<StreamId, Client<M, P, T, E>>,

    pub config: Arc<ServerConfig>,
    pub listener: TcpListener,
    pub poll: Poll,
    pub events: Events,
    pub heartbeat: Option<Duration>,
    pub last_beat: Instant, 
    pub events_processed: usize,
    pub id_to_delete: StreamId,
}

impl<M, P, T, E> Server_G<M, P, T, E> 
where 
    M: SendTo + Default,
    P: for<'b> Parser<'b, T, E> + Default,
    E: fmt::Display,
{
    pub fn new(address: SocketAddr, config: ServerConfig) -> Self {
        let poll = Poll::new().unwrap();
        let registry = poll.registry();
        let mut listener = TcpListener::bind(address).unwrap();

        registry.register(&mut listener, Token(SERVER), Interest::READABLE | Interest::WRITABLE).unwrap();

        println!("HTTPSERVER: initializing server on (https://{}:{})", address.ip(), address.port());

        Self { 
            clients: HashMap::with_capacity(1028),
            config: Arc::new(config),
            listener,
            poll,
            events: Events::with_capacity(128),
            heartbeat: None,
            last_beat: Instant::now(),
            events_processed: 0,
            id_to_delete: 0,
        }
    }
    pub fn serve(&mut self) -> Option<(StreamId, T)> {
        loop {
            let mut time = None;
            let non_blocking = self.heartbeat == Some(Duration::ZERO);
            if let Some(beat) = self.heartbeat {
                let timer = beat.saturating_sub(self.last_beat.elapsed());
                self.last_beat = Instant::now();
                if timer.is_zero() && !non_blocking {
                    return None
                }
                time = Some(timer)
            }
            if self.events.iter().nth(self.events_processed).is_none() {
                match self.poll.poll(&mut self.events, time) {
                    Ok(_) => {},
                    Err(ref e) if e.kind() == ErrorKind::Interrupted => {},
                    Err(e) => panic!("{e}"),
                }
                if non_blocking && self.events.is_empty() { 
                    return None
                }
            }
            while let Some(event) =  self.events.iter().nth(self.events_processed) {
                self.events_processed += 1;
                let id = event.token().0 as StreamId;
                match id {
                    SERVER => {
                        match self.listener.accept() {
                            Ok((client, _)) => {
                                self.register(client).unwrap();
                            }
                            Err(e) if e.kind() == ErrorKind::WouldBlock => {},
                            Err(e) => {
                                println!("TLServer: connection refused due to error accepting: {:?}", e);
                            },
                        }
                    }
                    _client => {
                        let client = self.clients.get_mut(&id).unwrap();
                        let stream = &mut client.stream;
                        if stream.tls.is_handshaking() {
                            match client.stream.handshake() {
                                Ok(_) => {},
                                Err(e) if e.kind() == ErrorKind::WouldBlock => {},
                                Err(e) if e.kind() == ErrorKind::Interrupted => {},
                                Err(e) => {
                                    println!("TLServer: dropped client on account of error when handshaking: {e}");
                                    self.drop_client(id);
                                },
                            };
                            continue
                        }
                        if event.is_writable() {
                            match stream.flush() {
                                Ok(_) => {},
                                Err(e) if e.kind() == ErrorKind::WouldBlock => {},
                                Err(e) if e.kind() == ErrorKind::ConnectionAborted => {
                                    self.drop_client(id);
                                    break
                                },
                                Err(e) => {
                                    println!("TLServer: dropped client on account of error when flushing: {e}");
                                    self.drop_client(id);
                                    break
                                },
                            } 
                            match client.messenger.send_all(stream) {
                                Ok(_) => {},
                                Err(e) if e.kind() == ErrorKind::WouldBlock => {},
                                Err(e) => {
                                    println!("HTTP_SERVER: dropped client on account of error when writing: {e}");
                                    self.drop_client(id);
                                    break
                                }
                            };
                        }
                        if event.is_readable() {
                            match throw_reader_at_writer(stream, &mut client.buf) {
                                Ok(()) => {},
                                Err(e) if e.kind() == ErrorKind::WouldBlock => {},
                                Err(e) => {
                                    println!("TLServer: dropped client on account of error when reading: {e}");
                                    self.drop_client(id);
                                    continue
                                }
                            };
                            if !client.buf.has_read() { continue }
                            let story = client.buf.the_story_so_far();
                            match client.parser.parse(story) {
                                Ok((Some(message), rest)) => {
                                    let old_len = client.buf.data.len();
                                    let new_len = rest.len();
                                    client.buf.data.copy_within(old_len - new_len.., 0);
                                    client.buf.data.resize(new_len, 0);
                                    client.buf.read = new_len;
                                    client.buf.prev_read = new_len;

                                    return Some((id, message))
                                },
                                Ok((None, rest)) => {
                                    if rest == client.buf.data {
                                        println!("TLServer: request incomplete message at size {}", story.len())
                                    }
                                    else {
                                        let old_len = client.buf.data.len();
                                        let new_len = rest.len();
                                        client.buf.data.copy_within(old_len-new_len.., 0);
                                        client.buf.data.resize(new_len, 0);
                                        client.buf.read = new_len;
                                        client.buf.prev_read = new_len;
                                        println!("TlServer: read {} bytes for handshake", old_len - new_len)
                                    }
                                },
                                Err(e) => {
                                    println!("TLServer: dropped client on account of error when parsing request: {e}");
                                    self.drop_client(id);
                                    break
                                },
                            }
                        }
                    }
                }
            }
        }
    }
    pub fn drop_client(&mut self, id: StreamId) {
        let client = self.clients.get_mut(&id).unwrap();
        
        let registry = self.poll.registry();
        registry.deregister(&mut client.stream).unwrap();

        self.clients.remove(&id).unwrap();
    }
    pub fn send_to_client(&mut self, id: StreamId, message: impl Into<M>) {
        let client = self.clients.get_mut(&id).unwrap();
        client.messenger = message.into();
        match client.messenger.send_all(&mut client.stream) {
            Ok(_) => {},
            Err(e) if e.kind() == ErrorKind::WouldBlock => {},
            Err(e) => {
                println!("HTTP_SERVER: dropped client on account of error when writing: {e}");
                self.drop_client(id);
            }
        };
    }
    fn register(&mut self, client: TcpStream) -> io::Result<StreamId> {
        let registry = self.poll.registry();
        let mut stream = TLStream::new(client, self.config.clone());
        let mut id = fastrand::usize(..);
        while self.clients.contains_key(&id) {
            id = fastrand::usize(..);
        }
        let token = Token(id as usize);
        let interests = Interest::READABLE | Interest::WRITABLE;
        registry.register(&mut stream, token, interests)?;

        let client = Client::new(id, stream);
        self.clients.insert(id, client);

        return Ok(id)
    }
}

impl<M, P, T, E> Iterator for &mut Server_G<M, P, T, E>
where 
    M: SendTo + Default,
    P: for<'b> Parser<'b, T, E> + Default,
    E: fmt::Display,
{
    type Item = (StreamId, T);
    fn next(&mut self) -> Option<Self::Item> {
        self.serve()
    }
}

pub struct Client<M, P, T, E>
where 
    M: SendTo + Default,
    P: for<'b> Parser<'b, T, E> + Default,
    E: fmt::Display,
{
    pub id: StreamId,
    pub stream: TLStream,
    pub buf: Buffer,
    pub messenger: M,
    pub parser: P,
    t: PhantomData<T>,
    e: PhantomData<E>,
}

impl<M, P, T, E> Client<M, P, T, E>
where 
    M: SendTo + Default,
    P: for<'b> Parser<'b, T, E> + Default,
    E: fmt::Display,
{
    fn new(id: StreamId, stream: TLStream) -> Self {
        Self {
            id,
            stream,
            buf: Buffer::with_capacity(4096), //TODO: maybe this should be less aligned?
            messenger: M::default(),
            parser: P::default(),
            t: PhantomData,
            e: PhantomData,
        }
    }
}