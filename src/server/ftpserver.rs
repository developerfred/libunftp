use super::chancomms::{InternalMsg, ProxyLoopMsg, ProxyLoopReceiver, ProxyLoopSender};
use super::controlchan::command::Command;
use super::controlchan::handler::{CommandContext, CommandHandler};
use super::controlchan::FTPCodec;
use super::controlchan::{ControlChanError, ControlChanErrorKind};
use super::io::*;
use super::proxy_protocol::*;
use super::*;
use super::{Reply, ReplyCode};
use super::{Session, SessionState};
use crate::auth::{anonymous::AnonymousAuthenticator, Authenticator, DefaultUser, UserDetail};
use crate::metrics;
use crate::server::session::SharedSession;
use crate::storage::{self, filesystem::Filesystem, ErrorKind};
use controlchan::commands;

use futures::channel::mpsc::{channel, Receiver, Sender};
use futures::{SinkExt, StreamExt};
use log::{error, info, warn};
use std::net::{IpAddr, Shutdown, SocketAddr};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio_util::codec::*;

const DEFAULT_GREETING: &str = "Welcome to the libunftp FTP server";
const DEFAULT_IDLE_SESSION_TIMEOUT_SECS: u64 = 600;

#[derive(Clone, Copy)]
struct ProxyParams {
    #[allow(dead_code)]
    external_ip: IpAddr,
    external_control_port: u16,
}

impl ProxyParams {
    fn new(ip: &str, port: u16) -> Result<Self, Box<dyn std::error::Error>> {
        Ok(ProxyParams {
            external_ip: ip.parse()?,
            external_control_port: port,
        })
    }
}

/// An instance of a FTP server. It contains a reference to an [`Authenticator`] that will be used
/// for authentication, and a [`StorageBackend`] that will be used as the storage backend.
///
/// The server can be started with the `listen` method.
///
/// # Example
///
/// ```rust
/// use libunftp::Server;
/// use tokio::runtime::Runtime;
///
/// let mut rt = Runtime::new().unwrap();
/// let server = Server::new_with_fs_root("/srv/ftp");
/// rt.spawn(server.listen("127.0.0.1:2121"));
/// // ...
/// drop(rt);
/// ```
///
/// [`Authenticator`]: auth/trait.Authenticator.html
/// [`StorageBackend`]: storage/trait.StorageBackend.html
pub struct Server<S, U>
where
    S: storage::StorageBackend<U> + Send + Sync,
    U: UserDetail,
{
    storage: Box<dyn (Fn() -> S) + Sync + Send>,
    greeting: &'static str,
    authenticator: Arc<dyn Authenticator<U> + Send + Sync>,
    passive_ports: Range<u16>,
    certs_file: Option<PathBuf>,
    certs_password: Option<String>,
    collect_metrics: bool,
    idle_session_timeout: std::time::Duration,
    proxy_protocol_mode: Option<ProxyParams>,
    proxy_protocol_switchboard: Option<ProxyProtocolSwitchboard<S, U>>,
}

impl Server<Filesystem, DefaultUser> {
    /// Create a new `Server` with the given filesystem root.
    ///
    /// # Example
    ///
    /// ```rust
    /// use libunftp::Server;
    ///
    /// let server = Server::new_with_fs_root("/srv/ftp");
    /// ```
    pub fn new_with_fs_root<P: Into<PathBuf> + Send + 'static>(path: P) -> Self {
        let p = path.into();
        Server::new(Box::new(move || {
            let p = &p.clone();
            storage::filesystem::Filesystem::new(p)
        }))
    }
}

impl<S, U> Server<S, U>
where
    S: 'static + storage::StorageBackend<U> + Sync + Send,
    S::File: tokio::io::AsyncRead + Send,
    S::Metadata: storage::Metadata,
    U: UserDetail + 'static,
{
    /// Construct a new [`Server`] with the given [`StorageBackend`]. The other parameters will be
    /// set to defaults.
    ///
    /// [`Server`]: struct.Server.html
    /// [`StorageBackend`]: ../storage/trait.StorageBackend.html
    pub fn new(s: Box<dyn (Fn() -> S) + Send + Sync>) -> Self
    where
        AnonymousAuthenticator: Authenticator<U>,
    {
        Server {
            storage: s,
            greeting: DEFAULT_GREETING,
            authenticator: Arc::new(AnonymousAuthenticator {}),
            passive_ports: 49152..65535,
            certs_file: Option::None,
            certs_password: Option::None,
            collect_metrics: false,
            idle_session_timeout: Duration::from_secs(DEFAULT_IDLE_SESSION_TIMEOUT_SECS),
            proxy_protocol_mode: Option::None,
            proxy_protocol_switchboard: Option::None,
        }
    }

    /// Construct a new [`Server`] with the given [`StorageBackend`] and [`Authenticator`]. The other parameters will be set to defaults.
    ///
    /// [`Server`]: struct.Server.html
    /// [`StorageBackend`]: ../storage/trait.StorageBackend.html
    /// [`Authenticator`]: ../auth/trait.Authenticator.html
    pub fn new_with_authenticator(s: Box<dyn (Fn() -> S) + Send + Sync>, authenticator: Arc<dyn Authenticator<U> + Send + Sync>) -> Self {
        Server {
            storage: s,
            greeting: DEFAULT_GREETING,
            authenticator,
            passive_ports: 49152..65535,
            certs_file: Option::None,
            certs_password: Option::None,
            collect_metrics: false,
            idle_session_timeout: Duration::from_secs(DEFAULT_IDLE_SESSION_TIMEOUT_SECS),
            proxy_protocol_mode: Option::None,
            proxy_protocol_switchboard: Option::None,
        }
    }

    /// Set the greeting that will be sent to the client after connecting.
    ///
    /// # Example
    ///
    /// ```rust
    /// use libunftp::Server;
    ///
    /// // Use it in a builder-like pattern:
    /// let mut server = Server::new_with_fs_root("/tmp").greeting("Welcome to my FTP Server");
    ///
    /// // Or instead if you prefer:
    /// let mut server = Server::new_with_fs_root("/tmp");
    /// server.greeting("Welcome to my FTP Server");
    /// ```
    pub fn greeting(mut self, greeting: &'static str) -> Self {
        self.greeting = greeting;
        self
    }

    /// Set the [`Authenticator`] that will be used for authentication.
    ///
    /// # Example
    ///
    /// ```rust
    /// use libunftp::{auth, auth::AnonymousAuthenticator, Server};
    /// use std::sync::Arc;
    ///
    /// // Use it in a builder-like pattern:
    /// let mut server = Server::new_with_fs_root("/tmp")
    ///                  .authenticator(Arc::new(auth::AnonymousAuthenticator{}));
    /// ```
    ///
    /// [`Authenticator`]: ../auth/trait.Authenticator.html
    pub fn authenticator(mut self, authenticator: Arc<dyn Authenticator<U> + Send + Sync>) -> Self {
        self.authenticator = authenticator;
        self
    }

    /// Set the range of passive ports that we'll use for passive connections.
    ///
    /// # Example
    ///
    /// ```rust
    /// use libunftp::Server;
    ///
    /// // Use it in a builder-like pattern:
    /// let mut server = Server::new_with_fs_root("/tmp").passive_ports(49152..65535);
    ///
    ///
    /// // Or instead if you prefer:
    /// let mut server = Server::new_with_fs_root("/tmp");
    /// server.passive_ports(49152..65535);
    /// ```
    pub fn passive_ports(mut self, range: Range<u16>) -> Self {
        self.passive_ports = range;
        self
    }

    /// Configures the path to the certificates file (DER-formatted PKCS #12 archive) and the
    /// associated password for the archive in order to configure FTPS.
    ///
    /// # Example
    ///
    /// ```rust
    /// use libunftp::Server;
    ///
    /// let mut server = Server::new_with_fs_root("/tmp").ftps("/srv/unftp/server-certs.pfx", "thepassword");
    /// ```
    pub fn ftps<P: Into<PathBuf>, T: Into<String>>(mut self, certs_file: P, password: T) -> Self {
        self.certs_file = Option::Some(certs_file.into());
        self.certs_password = Option::Some(password.into());
        self
    }

    /// Enable the collection of prometheus metrics.
    ///
    /// # Example
    ///
    /// ```rust
    /// use libunftp::Server;
    ///
    /// // Use it in a builder-like pattern:
    /// let mut server = Server::new_with_fs_root("/tmp").metrics();
    ///
    /// // Or instead if you prefer:
    /// let mut server = Server::new_with_fs_root("/tmp");
    /// server.metrics();
    /// ```
    pub fn metrics(mut self) -> Self {
        self.collect_metrics = true;
        self
    }

    /// Set the idle session timeout in seconds. The default is 600 seconds.
    ///
    /// # Example
    ///
    /// ```rust
    /// use libunftp::Server;
    ///
    /// // Use it in a builder-like pattern:
    /// let mut server = Server::new_with_fs_root("/tmp").idle_session_timeout(600);
    ///
    /// // Or instead if you prefer:
    /// let mut server = Server::new_with_fs_root("/tmp");
    /// server.idle_session_timeout(600);
    /// ```
    pub fn idle_session_timeout(mut self, secs: u64) -> Self {
        self.idle_session_timeout = Duration::from_secs(secs);
        self
    }

    /// Enable PROXY protocol mode.
    ///
    /// If you use a proxy such as haproxy or nginx, you can enable
    /// the PROXY protocol
    /// (https://www.haproxy.org/download/1.8/doc/proxy-protocol.txt).
    ///
    /// Configure your proxy to enable PROXY protocol encoding for
    /// control and data external listening ports, forwarding these
    /// connections to the libunFTP listening port in proxy protocol
    /// mode.
    ///
    /// In PROXY protocol mode, libunftp receives both control and
    /// data connections on the listening port. It then distinguishes
    /// control and data connections by comparing the original
    /// destination port (extracted from the PROXY header) with the
    /// port specified as the `external_control_port`
    /// `proxy_protocol_mode` parameter.
    ///
    /// For the passive listening port, libunftp reports the IP
    /// address specified as the `external_ip` `proxy_protocol_mode`
    /// parameter.
    ///
    /// # Example
    ///
    /// ```rust
    /// use libunftp::Server;
    ///
    /// // Use it in a builder-like pattern:
    /// let mut server = Server::new_with_fs_root("/tmp").proxy_protocol_mode("10.0.0.1", 2121).unwrap();
    /// ```
    pub fn proxy_protocol_mode(mut self, external_ip: &str, external_control_port: u16) -> Result<Self, Box<dyn std::error::Error>> {
        self.proxy_protocol_mode = Some(ProxyParams::new(external_ip, external_control_port)?);
        self.proxy_protocol_switchboard = Some(ProxyProtocolSwitchboard::new(self.passive_ports.clone()));

        Ok(self)
    }

    /// Runs the main ftp process asynchronously. Should be started in a async runtime context.
    ///
    /// # Example
    ///
    /// ```rust
    /// use libunftp::Server;
    /// use tokio::runtime::Runtime;
    ///
    /// let mut rt = Runtime::new().unwrap();
    /// let server = Server::new_with_fs_root("/srv/ftp");
    /// rt.spawn(server.listen("127.0.0.1:2121"));
    /// // ...
    /// drop(rt);
    /// ```
    ///
    /// # Panics
    ///
    /// This function panics when called with invalid addresses or when the process is unable to
    /// `bind()` to the address.
    pub async fn listen<T: Into<String>>(self, bind_address: T) {
        match self.proxy_protocol_mode {
            Some(_) => self.listen_proxy_protocol_mode(bind_address).await,
            None => self.listen_normal_mode(bind_address).await,
        }
    }

    async fn listen_normal_mode<T: Into<String>>(self, bind_address: T) {
        // TODO: Propagate errors to caller instead of doing unwraps.
        let addr: std::net::SocketAddr = bind_address.into().parse().unwrap();
        let mut listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        loop {
            let (tcp_stream, socket_addr) = listener.accept().await.unwrap();
            info!("Incoming control channel connection from {:?}", socket_addr);
            let result = self.spawn_control_channel_loop(tcp_stream, None, None).await;
            if result.is_err() {
                warn!("Could not spawn control channel loop for connection: {:?}", result.err().unwrap())
            }
        }
    }

    async fn listen_proxy_protocol_mode<T: Into<String>>(mut self, bind_address: T) {
        let proxy_params = self
            .proxy_protocol_mode
            .expect("You cannot use the proxy protocol listener without setting the proxy_protocol_mode parameters.");

        // TODO: Propagate errors to caller instead of doing unwraps.
        let addr: std::net::SocketAddr = bind_address.into().parse().unwrap();
        let mut listener = tokio::net::TcpListener::bind(addr).await.unwrap();

        // this callback is used by all sessions, basically only to
        // request for a passive listening port.
        let (proxyloop_msg_tx, mut proxyloop_msg_rx): (ProxyLoopSender<S, U>, ProxyLoopReceiver<S, U>) = channel(1);

        let mut incoming = listener.incoming();

        loop {
            // The 'proxy loop' handles two kinds of events:
            // - incoming tcp connections originating from the proxy
            // - channel messages originating from PASV, to handle the passive listening port

            tokio::select! {

                Some(tcp_stream) = incoming.next() => {
                    let mut tcp_stream = tcp_stream.unwrap();
                    let socket_addr = tcp_stream.peer_addr();

                    info!("Incoming proxy connection from {:?}", socket_addr);
                    let connection = match get_peer_from_proxy_header(&mut tcp_stream).await {
                        Ok(v) => v,
                        Err(e) => {
                            warn!("proxy protocol decode error: {:?}", e);
                            continue;
                        }
                    };

                    // Based on the proxy protocol header, and the configured control port number,
                    // we differentiate between connections for the control channel,
                    // and connections for the data channel.
                    if connection.to_port == proxy_params.external_control_port {
                        let socket_addr = SocketAddr::new(connection.from_ip, connection.from_port);
                        info!("Incoming control channel connection from {:?}", socket_addr);

                        let result = self.spawn_control_channel_loop(tcp_stream, Some(connection), Some(proxyloop_msg_tx.clone())).await;
                        if result.is_err() {
                            warn!("Could not spawn control channel loop for connection: {:?}", result.err().unwrap())
                        }
                    } else {
                        // handle incoming data connections
                        println!("{:?}, {}", self.passive_ports, connection.to_port);
                        if !self.passive_ports.contains(&connection.to_port) {
                            error!("Incoming proxy connection going to unconfigured port! This port is not configured as a passive listening port: port {} not in passive port range {:?}", connection.to_port, self.passive_ports);
                            tcp_stream.shutdown(Shutdown::Both).unwrap();
                            continue;
                        }

                        self.dispatch_data_connection(tcp_stream, connection).await;

                    }
                },
                Some(msg) = proxyloop_msg_rx.next() => {
                    match msg {
                        ProxyLoopMsg::AssignDataPortCommand (session_arc) => {
                            self.select_and_register_passive_port(session_arc).await;
                        },
                    }
                },
            };
        }
    }

    // this function finds (by hashing <srcip>.<dstport>) the session
    // that requested this data channel connection in the proxy
    // protocol switchboard hashmap, and then calls the
    // spawn_data_processing function with the tcp_stream
    async fn dispatch_data_connection(&mut self, tcp_stream: tokio::net::TcpStream, connection: ConnectionTuple) {
        if let Some(switchboard) = &mut self.proxy_protocol_switchboard {
            match switchboard.get_session_by_incoming_data_connection(&connection).await {
                Some(session) => {
                    let mut session = session.lock().await;
                    let tx_some = session.control_msg_tx.clone();
                    if let Some(tx) = tx_some {
                        datachan::spawn_processing(&mut session, tcp_stream, tx);
                        switchboard.unregister(&connection);
                    }
                }
                None => {
                    warn!("Unexpected connection ({:?})", connection);
                    tcp_stream.shutdown(Shutdown::Both).unwrap();
                    return;
                }
            }
        }
    }

    async fn select_and_register_passive_port(&mut self, session_arc: SharedSession<S, U>) {
        info!("Received command to allocate data port");
        // 1. reserve a port
        // 2. put the session_arc and tx in the hashmap with srcip+dstport as key
        // 3. put expiry time in the LIFO list
        // 4. send reply to client: "Entering Passive Mode ({},{},{},{},{},{})"

        let mut p1 = 0;
        let mut p2 = 0;
        if let Some(switchboard) = &mut self.proxy_protocol_switchboard {
            let port = switchboard.reserve_next_free_port(session_arc.clone()).await.unwrap();
            warn!("port: {:?}", port);
            p1 = port >> 8;
            p2 = port - (p1 * 256);
        }
        let session = session_arc.lock().await;
        if let Some(conn) = session.control_connection_info {
            let octets = match conn.from_ip {
                IpAddr::V4(ip) => ip.octets(),
                IpAddr::V6(_) => panic!("Won't happen."),
            };
            let tx_some = session.control_msg_tx.clone();
            if let Some(tx) = tx_some {
                let mut tx = tx.clone();
                tx.send(InternalMsg::CommandChannelReply(
                    ReplyCode::EnteringPassiveMode,
                    format!("Entering Passive Mode ({},{},{},{},{},{})", octets[0], octets[1], octets[2], octets[3], p1, p2),
                ))
                .await
                .unwrap();
            }
        }
    }

    /// Does TCP processing when a FTP client connects
    async fn spawn_control_channel_loop(
        &self,
        tcp_stream: tokio::net::TcpStream,
        control_connection_info: Option<ConnectionTuple>,
        proxyloop_msg_tx: Option<ProxyLoopSender<S, U>>,
    ) -> Result<(), ControlChanError> {
        let with_metrics = self.collect_metrics;
        let tls_configured = if let (Some(_), Some(_)) = (&self.certs_file, &self.certs_password) {
            true
        } else {
            false
        };
        let storage = Arc::new((self.storage)());
        let storage_features = storage.supported_features();
        let authenticator = self.authenticator.clone();
        let mut session = Session::new(storage)
            .ftps(self.certs_file.clone(), self.certs_password.clone())
            .metrics(with_metrics);
        let (control_msg_tx, control_msg_rx): (Sender<InternalMsg>, Receiver<InternalMsg>) = channel(1);
        session.control_msg_tx = Some(control_msg_tx.clone());
        session.control_connection_info = control_connection_info;
        let session = Arc::new(Mutex::new(session));
        let passive_ports = self.passive_ports.clone();
        let idle_session_timeout = self.idle_session_timeout;
        let local_addr = tcp_stream.local_addr().unwrap();
        let identity_file: Option<PathBuf> = if tls_configured {
            let p: PathBuf = self.certs_file.clone().unwrap();
            Some(p)
        } else {
            None
        };
        let identity_password: Option<String> = if tls_configured {
            let p: String = self.certs_password.clone().unwrap();
            Some(p)
        } else {
            None
        };

        let event_handler_chain = Self::handle_event(
            session.clone(),
            authenticator,
            tls_configured,
            passive_ports,
            control_msg_tx,
            local_addr,
            storage_features,
            proxyloop_msg_tx,
            control_connection_info,
        );
        let event_handler_chain = Self::handle_with_auth(session, event_handler_chain);
        let event_handler_chain = Self::handle_with_logging(event_handler_chain);

        let codec = FTPCodec::new();
        let cmd_and_reply_stream = codec.framed(tcp_stream.as_async_io());
        let (mut reply_sink, command_source) = cmd_and_reply_stream.split();

        reply_sink.send(Reply::new(ReplyCode::ServiceReady, self.greeting)).await?;
        reply_sink.flush().await?;

        let mut command_source = command_source.fuse();
        let mut control_msg_rx = control_msg_rx.fuse();

        tokio::spawn(async move {
            // The control channel event loop
            loop {
                #[allow(unused_assignments)]
                let mut incoming = None;
                let mut timeout_delay = tokio::time::delay_for(idle_session_timeout);
                tokio::select! {
                    Some(cmd_result) = command_source.next() => {
                        incoming = Some(cmd_result.map(Event::Command));
                    },
                    Some(msg) = control_msg_rx.next() => {
                        incoming = Some(Ok(Event::InternalMsg(msg)));
                    },
                    _ = &mut timeout_delay => {
                        info!("Connection timed out");
                        incoming = Some(Err(ControlChanError::new(ControlChanErrorKind::ControlChannelTimeout)));
                    }
                };

                match incoming {
                    None => {
                        // Should not happen.
                        warn!("No event polled...");
                        return;
                    }
                    Some(Ok(event)) => {
                        if with_metrics {
                            metrics::add_event_metric(&event);
                        };

                        if let Event::InternalMsg(InternalMsg::Quit) = event {
                            info!("Quit received");
                            return;
                        }

                        if let Event::InternalMsg(InternalMsg::SecureControlChannel) = event {
                            info!("Upgrading to TLS");

                            // Get back the original TCP Stream
                            let codec_io = reply_sink.reunite(command_source.into_inner()).unwrap();
                            let io = codec_io.into_inner();

                            // Wrap in TLS Stream
                            //let config = tls::new_config(&certs, &keys);
                            let identity = tls::identity(identity_file.clone().unwrap(), identity_password.clone().unwrap());
                            let acceptor = tokio_tls::TlsAcceptor::from(native_tls::TlsAcceptor::builder(identity).build().unwrap());
                            let io = acceptor.accept(io).await.unwrap().as_async_io();

                            // Wrap in codec again and get sink + source
                            let codec = controlchan::FTPCodec::new();
                            let cmd_and_reply_stream = codec.framed(io);
                            let (sink, src) = cmd_and_reply_stream.split();
                            let src = src.fuse();
                            reply_sink = sink;
                            command_source = src;
                        }

                        // TODO: Handle Event::InternalMsg(InternalMsg::PlaintextControlChannel)

                        match event_handler_chain(event) {
                            Err(e) => {
                                warn!("Event handler chain error: {:?}", e);
                                return;
                            }
                            Ok(reply) => {
                                if with_metrics {
                                    metrics::add_reply_metric(&reply);
                                }
                                let result = reply_sink.send(reply).await;
                                if result.is_err() {
                                    warn!("could not send reply");
                                    return;
                                }
                            }
                        }
                    }
                    Some(Err(e)) => {
                        let reply = Self::handle_control_channel_error(e, with_metrics);
                        let mut close_connection = false;
                        if let Reply::CodeAndMsg {
                            code: ReplyCode::ClosingControlConnection,
                            ..
                        } = reply
                        {
                            close_connection = true;
                        }
                        let result = reply_sink.send(reply).await;
                        if result.is_err() {
                            warn!("could not send error reply");
                            return;
                        }
                        if close_connection {
                            return;
                        }
                    }
                }
            }
        });

        Ok(())
    }

    fn handle_with_auth(
        session: SharedSession<S, U>,
        next: impl Fn(Event) -> Result<Reply, ControlChanError>,
    ) -> impl Fn(Event) -> Result<Reply, ControlChanError> {
        move |event| match event {
            // internal messages and the below commands are exempt from auth checks.
            Event::InternalMsg(_)
            | Event::Command(Command::Help)
            | Event::Command(Command::User { .. })
            | Event::Command(Command::Pass { .. })
            | Event::Command(Command::Auth { .. })
            | Event::Command(Command::Feat)
            | Event::Command(Command::Quit) => next(event),
            _ => {
                let r = futures::executor::block_on(async {
                    let session = session.lock().await;
                    if session.state != SessionState::WaitCmd {
                        Ok(Reply::new(ReplyCode::NotLoggedIn, "Please authenticate"))
                    } else {
                        Err(())
                    }
                });
                if let Ok(r) = r {
                    return Ok(r);
                }
                next(event)
            }
        }
    }

    fn handle_with_logging(next: impl Fn(Event) -> Result<Reply, ControlChanError>) -> impl Fn(Event) -> Result<Reply, ControlChanError> {
        move |event| {
            info!("Processing event {:?}", event);
            next(event)
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_event(
        session: SharedSession<S, U>,
        authenticator: Arc<dyn Authenticator<U> + Send + Sync>,
        tls_configured: bool,
        passive_ports: Range<u16>,
        tx: Sender<InternalMsg>,
        local_addr: std::net::SocketAddr,
        storage_features: u32,
        proxyloop_msg_tx: Option<ProxyLoopSender<S, U>>,
        control_connection_info: Option<ConnectionTuple>,
    ) -> impl Fn(Event) -> Result<Reply, ControlChanError> {
        move |event| -> Result<Reply, ControlChanError> {
            match event {
                Event::Command(cmd) => futures::executor::block_on(Self::handle_command(
                    cmd,
                    session.clone(),
                    authenticator.clone(),
                    tls_configured,
                    passive_ports.clone(),
                    tx.clone(),
                    local_addr,
                    storage_features,
                    proxyloop_msg_tx.clone(),
                    control_connection_info,
                )),
                Event::InternalMsg(msg) => futures::executor::block_on(Self::handle_internal_msg(msg, session.clone())),
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn handle_command(
        cmd: Command,
        session: SharedSession<S, U>,
        authenticator: Arc<dyn Authenticator<U>>,
        tls_configured: bool,
        passive_ports: Range<u16>,
        tx: Sender<InternalMsg>,
        local_addr: std::net::SocketAddr,
        storage_features: u32,
        proxyloop_msg_tx: Option<ProxyLoopSender<S, U>>,
        control_connection_info: Option<ConnectionTuple>,
    ) -> Result<Reply, ControlChanError> {
        let args = CommandContext {
            cmd: cmd.clone(),
            session,
            authenticator,
            tls_configured,
            passive_ports,
            tx,
            local_addr,
            storage_features,
            proxyloop_msg_tx,
            control_connection_info,
        };

        let handler: Box<dyn CommandHandler<S, U>> = match cmd {
            Command::User { username } => Box::new(commands::User::new(username)),
            Command::Pass { password } => Box::new(commands::Pass::new(password)),
            Command::Syst => Box::new(commands::Syst),
            Command::Stat { path } => Box::new(commands::Stat::new(path)),
            Command::Acct { .. } => Box::new(commands::Acct),
            Command::Type => Box::new(commands::Type),
            Command::Stru { structure } => Box::new(commands::Stru::new(structure)),
            Command::Mode { mode } => Box::new(commands::Mode::new(mode)),
            Command::Help => Box::new(commands::Help),
            Command::Noop => Box::new(commands::Noop),
            Command::Pasv => Box::new(commands::Pasv::new()),
            Command::Port => Box::new(commands::Port),
            Command::Retr { .. } => Box::new(commands::Retr),
            Command::Stor { .. } => Box::new(commands::Stor),
            Command::List { .. } => Box::new(commands::List),
            Command::Nlst { .. } => Box::new(commands::Nlst),
            Command::Feat => Box::new(commands::Feat),
            Command::Pwd => Box::new(commands::Pwd),
            Command::Cwd { path } => Box::new(commands::Cwd::new(path)),
            Command::Cdup => Box::new(commands::Cdup),
            Command::Opts { option } => Box::new(commands::Opts::new(option)),
            Command::Dele { path } => Box::new(commands::Dele::new(path)),
            Command::Rmd { path } => Box::new(commands::Rmd::new(path)),
            Command::Quit => Box::new(commands::Quit),
            Command::Mkd { path } => Box::new(commands::Mkd::new(path)),
            Command::Allo { .. } => Box::new(commands::Allo),
            Command::Abor => Box::new(commands::Abor),
            Command::Stou => Box::new(commands::Stou),
            Command::Rnfr { file } => Box::new(commands::Rnfr::new(file)),
            Command::Rnto { file } => Box::new(commands::Rnto::new(file)),
            Command::Auth { protocol } => Box::new(commands::Auth::new(protocol)),
            Command::PBSZ {} => Box::new(commands::Pbsz),
            Command::CCC {} => Box::new(commands::Ccc),
            Command::PROT { param } => Box::new(commands::Prot::new(param)),
            Command::SIZE { file } => Box::new(commands::Size::new(file)),
            Command::Rest { offset } => Box::new(commands::Rest::new(offset)),
            Command::MDTM { file } => Box::new(commands::Mdtm::new(file)),
        };

        handler.handle(args).await
    }

    async fn handle_internal_msg(msg: InternalMsg, session: SharedSession<S, U>) -> Result<Reply, ControlChanError> {
        use self::InternalMsg::*;
        use SessionState::*;

        match msg {
            NotFound => Ok(Reply::new(ReplyCode::FileError, "File not found")),
            PermissionDenied => Ok(Reply::new(ReplyCode::FileError, "Permision denied")),
            SendingData => Ok(Reply::new(ReplyCode::FileStatusOkay, "Sending Data")),
            SendData { .. } => {
                let mut session = session.lock().await;
                session.start_pos = 0;
                Ok(Reply::new(ReplyCode::ClosingDataConnection, "Successfully sent"))
            }
            WriteFailed => Ok(Reply::new(ReplyCode::TransientFileError, "Failed to write file")),
            ConnectionReset => Ok(Reply::new(ReplyCode::ConnectionClosed, "Datachannel unexpectedly closed")),
            WrittenData { .. } => {
                let mut session = session.lock().await;
                session.start_pos = 0;
                Ok(Reply::new(ReplyCode::ClosingDataConnection, "File successfully written"))
            }
            DataConnectionClosedAfterStor => Ok(Reply::new(ReplyCode::FileActionOkay, "unFTP holds your data for you")),
            UnknownRetrieveError => Ok(Reply::new(ReplyCode::TransientFileError, "Unknown Error")),
            DirectorySuccessfullyListed => Ok(Reply::new(ReplyCode::ClosingDataConnection, "Listed the directory")),
            CwdSuccess => Ok(Reply::new(ReplyCode::FileActionOkay, "Successfully cwd")),
            DelSuccess => Ok(Reply::new(ReplyCode::FileActionOkay, "File successfully removed")),
            DelFail => Ok(Reply::new(ReplyCode::TransientFileError, "Failed to delete the file")),
            // The InternalMsg::Quit will never be reached, because we catch it in the task before
            // this closure is called (because we have to close the connection).
            Quit => Ok(Reply::new(ReplyCode::ClosingControlConnection, "Bye!")),
            SecureControlChannel => {
                let mut session = session.lock().await;
                session.cmd_tls = true;
                Ok(Reply::none())
            }
            PlaintextControlChannel => {
                let mut session = session.lock().await;
                session.cmd_tls = false;
                Ok(Reply::none())
            }
            MkdirSuccess(path) => Ok(Reply::new_with_string(ReplyCode::DirCreated, path.to_string_lossy().to_string())),
            MkdirFail => Ok(Reply::new(ReplyCode::FileError, "Failed to create directory")),
            AuthSuccess => {
                let mut session = session.lock().await;
                session.state = WaitCmd;
                Ok(Reply::new(ReplyCode::UserLoggedIn, "User logged in, proceed"))
            }
            AuthFailed => Ok(Reply::new(ReplyCode::NotLoggedIn, "Authentication failed")),
            StorageError(error_type) => match error_type.kind() {
                ErrorKind::ExceededStorageAllocationError => Ok(Reply::new(ReplyCode::ExceededStorageAllocation, "Exceeded storage allocation")),
                ErrorKind::FileNameNotAllowedError => Ok(Reply::new(ReplyCode::BadFileName, "File name not allowed")),
                ErrorKind::InsufficientStorageSpaceError => Ok(Reply::new(ReplyCode::OutOfSpace, "Insufficient storage space")),
                ErrorKind::LocalError => Ok(Reply::new(ReplyCode::LocalError, "Local error")),
                ErrorKind::PageTypeUnknown => Ok(Reply::new(ReplyCode::PageTypeUnknown, "Page type unknown")),
                ErrorKind::TransientFileNotAvailable => Ok(Reply::new(ReplyCode::TransientFileError, "File not found")),
                ErrorKind::PermanentFileNotAvailable => Ok(Reply::new(ReplyCode::FileError, "File not found")),
                ErrorKind::PermissionDenied => Ok(Reply::new(ReplyCode::FileError, "Permission denied")),
            },
            CommandChannelReply(reply_code, message) => Ok(Reply::new(reply_code, &message)),
        }
    }

    fn handle_control_channel_error(error: ControlChanError, with_metrics: bool) -> Reply {
        if with_metrics {
            metrics::add_error_metric(&error.kind());
        };
        warn!("Control channel error: {}", error);
        match error.kind() {
            ControlChanErrorKind::UnknownCommand { .. } => Reply::new(ReplyCode::CommandSyntaxError, "Command not implemented"),
            ControlChanErrorKind::UTF8Error => Reply::new(ReplyCode::CommandSyntaxError, "Invalid UTF8 in command"),
            ControlChanErrorKind::InvalidCommand => Reply::new(ReplyCode::ParameterSyntaxError, "Invalid Parameter"),
            ControlChanErrorKind::ControlChannelTimeout => Reply::new(ReplyCode::ClosingControlConnection, "Session timed out. Closing control connection"),
            _ => Reply::new(ReplyCode::LocalError, "Unknown internal server error, please try again later"),
        }
    }
}
