/*
 * Steps of XWayland server creation
 *
 * Sockets to create:
 * - a pair for XWayland to connect to smithay as a wayland client, we use our
 *   end to insert the XWayland client in the display
 * - a pair for smithay to connect to XWayland as a WM, we give our end to the
 *   WM and it deals with it
 * - 2 listening sockets on which the XWayland server will listen. We need to
 *   bind them ourselves so we know what value put in the $DISPLAY env variable.
 *   This involves some dance with a lockfile to ensure there is no collision with
 *   an other starting xserver
 *   if we listen on display $D, their paths are respectively:
 *   - /tmp/.X11-unix/X$D
 *   - @/tmp/.X11-unix/X$D (abstract socket)
 *
 * The XWayland server is spawned via an intermediate shell
 * -> wlroots does a double-fork while weston a single one, why ??
 *    -> https://stackoverflow.com/questions/881388/
 * -> once it is started, it will check if SIGUSR1 is set to ignored. If so,
 *    if will consider its parent as "smart", and send a SIGUSR1 signal when
 *    startup completes. We want to catch this so we can launch the VM.
 * -> we need to track if the XWayland crashes, to restart it
 *
 * cf https://github.com/swaywm/wlroots/blob/master/xwayland/xwayland.c
 *
 * Setting SIGUSR1 handler is complicated in multithreaded program, because
 * Xwayland will send SIGUSR1 to the process, and if a thread cannot handle
 * SIGUSR1, that thread will be killed.
 *
 * Double-fork can tackle this issue, but this is also very complex in a
 * a multithread program, after forking only signal-safe functions can be used.
 * The only workaround is to fork early before any other thread starts, but
 * doing so will expose an unsafe interface.
 *
 * We use an intermediate shell to translate the signal to simple fd IO.
 * We ask sh to setup SIGUSR1 handler, and in a subshell mute SIGUSR1 and exec
 * Xwayland. When the SIGUSR1 is received, it can communicate to us via redirected
 * STDOUT.
 */
use std::{
    env,
    fmt::Write,
    io::{self, Read},
    os::unix::{
        io::{AsRawFd, RawFd},
        net::UnixStream,
        process::CommandExt,
    },
    process::{ChildStdout, Command, Stdio},
    sync::{Arc, Mutex},
};

use calloop::{
    channel::{self, sync_channel, Channel, SyncSender},
    generic::Generic,
    Interest, LoopHandle, Mode,
};
use wayland_server::{
    backend::{ClientData, ClientId, DisconnectReason},
    Client, DisplayHandle,
};

use slog::{error, info, o};

use super::x11_sockets::{prepare_x11_sockets, X11Lock};

/// The XWayland handle
#[derive(Debug)]
pub struct XWayland {
    inner: Arc<Mutex<Inner>>,
}

/// Events generated by the XWayland manager
///
/// This is a very low-level interface, only notifying you when the connection
/// with XWayland is up, or when it terminates.
///
/// Your WM code must be able to handle the XWayland server connecting then
/// disconnecting several time in a row, but only a single connection will
/// be active at any given time.
#[derive(Debug)]
#[must_use = "Connection events must be handled to prevent fd leaking"]
pub enum XWaylandEvent {
    /// The XWayland server is ready
    Ready {
        /// Privileged X11 connection to XWayland
        connection: UnixStream,

        /// Wayland client representing XWayland
        client: Client,

        /// Wayland client file descriptor in case you are not using the display's poll_fd
        client_fd: RawFd,

        /// The display number the XWayland server is available at.
        ///
        /// This may be used if you wish to set the `DISPLAY` variable manually when spawning processes that
        /// may use XWayland.
        display: u32,
    },

    /// The XWayland server exited
    ///
    /// This event is sent when the [`XWayland`] handle is dropped.
    Exited,
}

impl XWayland {
    /// Create a new XWayland manager
    ///
    /// This function returns both the [`XWayland`] handle and an [`XWaylandSource`] that needs to be inserted
    /// into the [`calloop`] event loop, producing the Xwayland startup and shutdown events.
    pub fn new<L>(logger: L, dh: &DisplayHandle) -> (XWayland, XWaylandSource)
    where
        L: Into<Option<::slog::Logger>>,
    {
        let log = crate::slog_or_fallback(logger);
        // We don't expect to ever have more than 2 messages in flight, if XWayland got ready and then died right away
        let (sender, channel) = sync_channel(2);
        let inner = Arc::new(Mutex::new(Inner {
            instance: None,
            sender,
            dh: dh.clone(),
            log: log.new(o!("smithay_module" => "XWayland")),
        }));
        (XWayland { inner }, XWaylandSource { channel })
    }

    /// Attempt to start the XWayland instance
    ///
    /// If it succeeds, you'll eventually receive an `XWaylandEvent::Ready`
    /// through the source provided by `XWayland::new()` containing an
    /// `UnixStream` representing your WM connection to XWayland, and the
    /// wayland `Client` for XWayland.
    ///
    /// Does nothing if XWayland is already started or starting.
    pub fn start<D>(&self, loop_handle: LoopHandle<'_, D>) -> io::Result<()> {
        let dh = self.inner.lock().unwrap().dh.clone();
        launch(&self.inner, loop_handle, dh)
    }

    /// Shutdown XWayland
    ///
    /// Does nothing if it was not already running, otherwise kills it and you will
    /// later receive a `XWaylandEvent::Exited` event.
    pub fn shutdown(&self) {
        self.inner.lock().unwrap().shutdown();
    }
}

impl Drop for XWayland {
    fn drop(&mut self) {
        self.inner.lock().unwrap().shutdown();
    }
}

#[derive(Debug)]
struct XWaylandInstance {
    display_lock: X11Lock,
    wayland_client: Option<Client>,
    wayland_client_fd: Option<RawFd>,
    wm_fd: Option<UnixStream>,
    child_stdout: ChildStdout,
}

// Inner implementation of the XWayland manager
#[derive(Debug)]
struct Inner {
    sender: SyncSender<XWaylandEvent>,
    instance: Option<XWaylandInstance>,
    dh: DisplayHandle,
    log: ::slog::Logger,
}

struct XWaylandClientData {
    inner: Arc<Mutex<Inner>>,
}

impl ClientData for XWaylandClientData {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {
        // If we are unable to take a lock we are most likely called during
        // a shutdown. This will definitely be the case when the compositor exits
        // and the XWayland instance is dropped.
        if let Ok(mut guard) = self.inner.try_lock() {
            guard.shutdown();
        }
    }
}

// Launch an XWayland server
//
// Does nothing if there is already a launched instance
fn launch<D>(
    inner: &Arc<Mutex<Inner>>,
    loop_handle: LoopHandle<'_, D>,
    mut dh: DisplayHandle,
) -> io::Result<()> {
    let mut guard = inner.lock().unwrap();
    if guard.instance.is_some() {
        return Ok(());
    }

    info!(guard.log, "Starting XWayland");

    let (x_wm_x11, x_wm_me) = UnixStream::pair()?;
    let (wl_x11, wl_me) = UnixStream::pair()?;

    let (lock, x_fds) = prepare_x11_sockets(guard.log.clone())?;

    // we have now created all the required sockets

    // all is ready, we can do the fork dance
    let child_stdout = match spawn_xwayland(lock.display(), wl_x11, x_wm_x11, &x_fds) {
        Ok(child_stdout) => child_stdout,
        Err(e) => {
            error!(guard.log, "XWayland failed to spawn"; "err" => format!("{:?}", e));
            return Err(e);
        }
    };

    let loop_inner = inner.clone();
    loop_handle
        .insert_source(
            Generic::<_, io::Error>::new(child_stdout.as_raw_fd(), Interest::READ, Mode::Level),
            move |_, _, _| {
                // the closure must be called exactly one time, this cannot panic
                xwayland_ready(&loop_inner);
                Ok(calloop::PostAction::Remove)
            },
        )
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;

    let client_fd = wl_me.as_raw_fd();
    let client = dh.insert_client(wl_me, Arc::new(XWaylandClientData { inner: inner.clone() }))?;
    guard.instance = Some(XWaylandInstance {
        display_lock: lock,
        wayland_client: Some(client),
        wayland_client_fd: Some(client_fd),
        wm_fd: Some(x_wm_me),
        child_stdout,
    });

    Ok(())
}

/// An event source for monitoring XWayland status
///
/// You need to insert it in a [`calloop`] event loop to handle the events it produces,
/// of type [`XWaylandEvent`], which notify you about startup and shutdown of the Xwayland
/// instance.
#[derive(Debug)]
#[must_use]
pub struct XWaylandSource {
    channel: Channel<XWaylandEvent>,
}

impl calloop::EventSource for XWaylandSource {
    type Event = XWaylandEvent;
    type Metadata = ();
    type Ret = ();
    type Error = io::Error;

    fn process_events<F>(
        &mut self,
        readiness: calloop::Readiness,
        token: calloop::Token,
        mut callback: F,
    ) -> io::Result<calloop::PostAction>
    where
        F: FnMut(Self::Event, &mut Self::Metadata) -> Self::Ret,
    {
        self.channel
            .process_events(readiness, token, |event, &mut ()| match event {
                channel::Event::Msg(msg) => callback(msg, &mut ()),
                channel::Event::Closed => {}
            })
            .map_err(|err| io::Error::new(io::ErrorKind::BrokenPipe, err))
    }

    fn register(
        &mut self,
        poll: &mut calloop::Poll,
        factory: &mut calloop::TokenFactory,
    ) -> calloop::Result<()> {
        self.channel.register(poll, factory)
    }

    fn reregister(
        &mut self,
        poll: &mut calloop::Poll,
        factory: &mut calloop::TokenFactory,
    ) -> calloop::Result<()> {
        self.channel.reregister(poll, factory)
    }

    fn unregister(&mut self, poll: &mut calloop::Poll) -> calloop::Result<()> {
        self.channel.unregister(poll)
    }
}

impl Inner {
    // Shutdown the XWayland server and cleanup everything
    fn shutdown(&mut self) {
        // don't do anything if not running
        if let Some(instance) = self.instance.take() {
            info!(self.log, "Shutting down XWayland.");
            if let Some(client) = instance.wayland_client {
                self.dh
                    .backend_handle()
                    .kill_client(client.id(), DisconnectReason::ConnectionClosed);
            }

            // send error occurs if the user dropped the channel... We cannot do much except ignore.
            let _ = self.sender.send(XWaylandEvent::Exited);

            // All connections and lockfiles are cleaned by their destructors

            // Remove DISPLAY from the env
            ::std::env::remove_var("DISPLAY");
            // We do like wlroots:
            // > We do not kill the XWayland process, it dies to broken pipe
            // > after we close our side of the wm/wl fds. This is more reliable
            // > than trying to kill something that might no longer be XWayland.
        }
    }
}

fn xwayland_ready(inner: &Arc<Mutex<Inner>>) {
    // Lots of re-borrowing to please the borrow-checker
    let mut guard = inner.lock().unwrap();
    let guard = &mut *guard;
    info!(guard.log, "XWayland ready");
    // instance should never be None at this point
    let instance = guard.instance.as_mut().unwrap();
    // neither the child_stdout
    let child_stdout = &mut instance.child_stdout;

    // This reads the one byte that is written when sh receives SIGUSR1
    let mut buffer = [0];
    let success = match child_stdout.read(&mut buffer) {
        Ok(len) => len > 0 && buffer[0] == b'S',
        Err(e) => {
            error!(guard.log, "Checking launch status failed"; "err" => format!("{:?}", e));
            false
        }
    };

    if success {
        // setup the environment
        ::std::env::set_var("DISPLAY", format!(":{}", instance.display_lock.display()));

        // signal the WM
        info!(
            guard.log,
            "XWayland is ready on DISPLAY \":{}\", signaling the WM.",
            instance.display_lock.display()
        );
        // send error occurs if the user dropped the channel... We cannot do much except ignore.
        let _ = guard.sender.send(XWaylandEvent::Ready {
            connection: instance.wm_fd.take().unwrap(), // This is a bug if None
            client: instance.wayland_client.take().unwrap(), // TODO: .clone().unwrap(),
            client_fd: instance.wayland_client_fd.take().unwrap(),
            display: instance.display_lock.display(),
        });
    } else {
        error!(
            guard.log,
            "XWayland crashed at startup, will not try to restart it."
        );
    }
}

/// Spawn XWayland with given sockets on given display
///
/// Returns a pipe that outputs 'S' upon successful launch.
fn spawn_xwayland(
    display: u32,
    wayland_socket: UnixStream,
    wm_socket: UnixStream,
    listen_sockets: &[UnixStream],
) -> io::Result<ChildStdout> {
    let mut command = Command::new("sh");

    // We use output stream to communicate because FD is easier to handle than exit code.
    command.stdout(Stdio::piped());

    let mut xwayland_args = format!(":{} -rootless -terminate -wm {}", display, wm_socket.as_raw_fd());
    for socket in listen_sockets {
        // Will only fail to write on OOM, so this panic is fine.
        write!(xwayland_args, " -listenfd {}", socket.as_raw_fd()).unwrap();
    }
    // This command let sh to:
    // * Set up signal handler for USR1
    // * Launch Xwayland with USR1 ignored so Xwayland will signal us when it is ready (also redirect
    //   Xwayland's STDOUT to STDERR so its output, if any, won't distract us)
    // * Print "S" and exit if USR1 is received
    command.arg("-c").arg(format!(
        "trap 'echo S' USR1; (trap '' USR1; exec Xwayland {}) 1>&2 & wait",
        xwayland_args
    ));

    // Setup the environment: clear everything except PATH and XDG_RUNTIME_DIR
    command.env_clear();
    for (key, value) in env::vars_os() {
        if key.to_str() == Some("PATH") || key.to_str() == Some("XDG_RUNTIME_DIR") {
            command.env(key, value);
            continue;
        }
    }
    command.env("WAYLAND_SOCKET", format!("{}", wayland_socket.as_raw_fd()));

    unsafe {
        let wayland_socket_fd = wayland_socket.as_raw_fd();
        let wm_socket_fd = wm_socket.as_raw_fd();
        let socket_fds: Vec<_> = listen_sockets.iter().map(|socket| socket.as_raw_fd()).collect();
        command.pre_exec(move || {
            // unset the CLOEXEC flag from the sockets we need to pass
            // to xwayland
            unset_cloexec(wayland_socket_fd)?;
            unset_cloexec(wm_socket_fd)?;
            for &socket in socket_fds.iter() {
                unset_cloexec(socket)?;
            }
            Ok(())
        });
    }

    let mut child = command.spawn()?;
    Ok(child.stdout.take().expect("stdout should be piped"))
}

/// Remove the `O_CLOEXEC` flag from this `Fd`
///
/// This means that the `Fd` will *not* be automatically
/// closed when we `exec()` into XWayland
fn unset_cloexec(fd: RawFd) -> io::Result<()> {
    use nix::fcntl::{fcntl, FcntlArg, FdFlag};
    fcntl(fd, FcntlArg::F_SETFD(FdFlag::empty()))?;
    Ok(())
}
