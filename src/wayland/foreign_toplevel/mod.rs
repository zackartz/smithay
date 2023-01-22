#![allow(missing_docs)]

mod dispatch;

use std::{
    collections::HashMap,
    fmt, hash,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc, Mutex,
    },
};

use wayland_backend::server::{ClientId, GlobalId};
use wayland_server::{protocol::wl_output, Client, Dispatch, DisplayHandle, GlobalDispatch, Resource};

use crate::utils::{DeadResource, DoubleBufferable, DoubleBuffered};

use self::{generated::ext_foreign_toplevel_handle_v1, protocol::ext_foreign_toplevel_info_v1};

#[derive(Debug)]
pub struct ForeignToplevelInfo {
    global: GlobalId,
    clients: HashMap<ClientId, ForeignToplevelClient>,
    toplevels: Vec<ToplevelHandle>,
    display: DisplayHandle,
}

impl ForeignToplevelInfo {
    pub fn new<D, F, L>(display: &DisplayHandle, filter: F, _logger: L) -> Self
    where
        D: GlobalDispatch<ext_foreign_toplevel_info_v1::ExtForeignToplevelInfoV1, GlobalData>
            + Dispatch<ext_foreign_toplevel_info_v1::ExtForeignToplevelInfoV1, ()>
            + Dispatch<ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1, ToplevelHandleData>
            + ForeignToplevelInfoHandler
            + 'static,
        F: Fn(&Client) -> bool + Send + Sync + 'static,
        L: Into<Option<::slog::Logger>>,
    {
        let global = display.create_global::<D, ext_foreign_toplevel_info_v1::ExtForeignToplevelInfoV1, _>(
            VERSION,
            GlobalData {
                filter: Box::new(filter),
            },
        );

        Self {
            global,
            clients: HashMap::default(),
            toplevels: Vec::new(),
            display: display.clone(),
        }
    }

    pub fn get_client(&self, client: &Client) -> Option<ForeignToplevelClient> {
        self.clients.get(&client.id()).cloned()
    }

    /// Creates a toplevel handle and advertises the toplevel to all clients.
    ///
    /// This does not configure the state of the toplevels when sent. The compositor must set and send the state
    /// tracked by the handle.
    #[must_use = "the toplevel state must be configured"]
    pub fn create_handle<State>(&mut self) -> ToplevelHandle
    where
        State: Dispatch<ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1, ToplevelHandleData>
            + 'static,
    {
        let handle = ToplevelHandle {
            inner: Arc::new(ToplevelHandleInner {
                handles: Mutex::new(Vec::new()),
                state: Mutex::new(DoubleBuffered::new()),
                initialized: AtomicBool::new(false),
            }),
        };
        self.toplevels.push(handle.clone());

        for client in self.clients.values() {
            handle.create_for_client::<State>(&self.display, client);
        }

        handle
    }

    /// Destroy the toplevel handle.
    ///
    /// When this occurs, any existing handles are sent the `closed` event.
    pub fn destroy_handle(&mut self, handle: &ToplevelHandle) {
        self.toplevels.retain(|other| other != handle);

        for handle in handle.inner.handles.lock().unwrap().iter() {
            handle.closed();
        }
    }

    /// Returns the id of the [`ExtForeignToplevelInfoV1`](ext_foreign_toplevel_info_v1::ExtForeignToplevelInfoV1) global.
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }
}

pub trait ForeignToplevelInfoHandler {
    fn foreign_toplevel(&mut self) -> &mut ForeignToplevelInfo;

    fn new_client(&mut self, client: ForeignToplevelClient);

    fn client_destroyed(&mut self, client: &ForeignToplevelClient);
}

pub struct GlobalData {
    filter: Box<dyn Fn(&Client) -> bool + Send + Sync>,
}

impl fmt::Debug for GlobalData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GlobalData").finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub struct ForeignToplevelInfoData {
    info: Arc<ClientData>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignToplevelClient {
    object: ext_foreign_toplevel_info_v1::ExtForeignToplevelInfoV1,
}

impl ForeignToplevelClient {
    pub fn info(&self) -> ext_foreign_toplevel_info_v1::ExtForeignToplevelInfoV1 {
        self.object.clone()
    }

    /// Tell the client that the compositor will no longer send events.
    pub fn finish(&self) {
        let data = self.data();

        if !data.info.stop.fetch_and(true, Ordering::SeqCst) {
            self.object.finished();
        }
    }

    fn data(&self) -> &ForeignToplevelInfoData {
        self.object.data::<ForeignToplevelInfoData>().unwrap()
    }
}

impl hash::Hash for ForeignToplevelClient {
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        self.object.id().hash(state);
    }
}

#[derive(Debug, Clone)]
pub struct ToplevelHandle {
    inner: Arc<ToplevelHandleInner>,
}

impl PartialEq for ToplevelHandle {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl ToplevelHandle {
    pub fn from_handle(
        handle: &ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1,
    ) -> Result<Self, DeadResource> {
        let data = handle.data::<ToplevelHandleData>().ok_or(DeadResource)?;
        let handles = data.inner.handles.lock().unwrap();

        if handles.iter().any(|other| other == handle) {
            return Ok(Self {
                inner: data.inner.clone(),
            });
        }

        Err(DeadResource)
    }

    pub fn pending_state<F, T>(&self, f: F) -> T
    where
        F: FnOnce(&mut ToplevelState) -> T,
    {
        let mut state = self.inner.state.lock().unwrap();
        f(state.pending_mut())
    }

    pub fn send_state(&self) {
        self.inner.send_state();
    }

    pub fn handle_for_client(
        &self,
        client: &Client,
    ) -> Result<ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1, DeadResource> {
        self.inner.handle_for_client(client)
    }

    fn create_for_client<State>(
        &self,
        display: &DisplayHandle,
        toplevel_client: &ForeignToplevelClient,
    ) -> Option<ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1>
    where
        State: Dispatch<ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1, ToplevelHandleData>
            + 'static,
    {
        let toplevel_client_info = &toplevel_client.data().info;
        // If the client is not receiving any more new toplevels, do not send new toplevels to it.
        if toplevel_client_info.stop.load(Ordering::Relaxed) {
            return None;
        }

        let client = &toplevel_client_info.client;
        // The new resource must be sent to the client immediately to prevent client side state corruption.
        let handle = client
            .create_resource::<_, _, State>(
                display,
                VERSION,
                ToplevelHandleData {
                    inner: self.inner.clone(),
                },
            )
            .unwrap();
        toplevel_client.object.toplevel(&handle);

        toplevel_client
            .data()
            .info
            .toplevel_count
            .fetch_add(1, Ordering::Relaxed);
        let mut handles = self.inner.handles.lock().unwrap();
        handles.push(handle.clone());

        Some(handle)
    }
}

#[derive(Debug, Default, Clone)]
pub struct ToplevelStateSet {
    inner: Vec<ext_foreign_toplevel_handle_v1::State>,
}

impl ToplevelStateSet {
    /// Returns `true` if the states contains a state.
    pub fn contains(&self, state: ext_foreign_toplevel_handle_v1::State) -> bool {
        self.inner.iter().any(|&s| s == state)
    }

    /// Adds a state to the states.
    ///
    /// If the states did not have this state present, `true` is returned.
    ///
    /// If the states did have this state present, `false` is returned.
    pub fn set(&mut self, state: ext_foreign_toplevel_handle_v1::State) -> bool {
        if self.contains(state) {
            false
        } else {
            self.inner.push(state);
            true
        }
    }

    /// Removes a state from the states. Returns whether the state was
    /// present in the states.
    pub fn unset(&mut self, state: ext_foreign_toplevel_handle_v1::State) -> bool {
        if !self.contains(state) {
            false
        } else {
            self.inner.retain(|&s| s != state);
            true
        }
    }

    pub fn raw(&self) -> Vec<u8> {
        self.inner
            .iter()
            .copied()
            .map(Into::<u32>::into)
            .map(u32::to_le_bytes)
            .flatten()
            .collect()
    }
}

impl PartialEq for ToplevelStateSet {
    fn eq(&self, other: &Self) -> bool {
        self.inner.iter().all(|&state| other.contains(state))
    }
}
impl Eq for ToplevelStateSet {}

#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct ToplevelState {
    pub title: String,
    pub app_id: String,
    pub outputs: Vec<wl_output::WlOutput>,
    pub states: ToplevelStateSet,
    pub parent: Option<ToplevelHandle>,
}

impl DoubleBufferable for ToplevelState {
    fn merge_into(&self, into: &mut Self) {
        // TODO: Better merge logic in the future
        *into = self.clone();
    }
}

#[derive(Debug)]
pub struct ToplevelHandleData {
    inner: Arc<ToplevelHandleInner>,
}

#[derive(Debug)]
struct ToplevelHandleInner {
    handles: Mutex<Vec<ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1>>,
    state: Mutex<DoubleBuffered<ToplevelState>>,
    initialized: AtomicBool,
}

impl ToplevelHandleInner {
    fn handle_for_client(
        &self,
        client: &Client,
    ) -> Result<ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1, DeadResource> {
        let guard = self.handles.lock().unwrap();
        let handles = guard.clone();
        drop(guard); // drop guard otherwise looking up the parent handle will deadlock.
        handles
            .iter()
            .find(|handle| handle.client().as_ref() == Some(client))
            .cloned()
            .ok_or(DeadResource)
    }

    fn send_state(&self) {
        // mark the state as initialized for future new clients
        self.initialized.store(true, Ordering::Relaxed);
        let handles = self.handles.lock().unwrap();

        for handle in handles.iter() {
            self.update_state(handle);
        }

        // Finally commit the current state after updating each client's current state.
        self.state.lock().unwrap().apply_pending();
    }

    fn update_state(&self, handle: &ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1) {
        let state = self.state.lock().unwrap();
        let pending = state.pending();
        let current = state.current();

        if Some(&pending.title) != current.map(|state| &state.title) {
            handle.title(pending.title.clone());
        }

        if Some(&pending.app_id) != current.map(|state| &state.app_id) {
            handle.app_id(pending.app_id.clone());
        }

        let mut new_outputs = Vec::new();
        let mut old_outputs = Vec::new();

        match current {
            Some(current) => {
                // Find all outputs that were added in the pending state
                let new = pending
                    .outputs
                    .iter()
                    .filter(|&output| !current.outputs.contains(output));
                // Find all outputs removed from the current state.
                let old = current
                    .outputs
                    .iter()
                    .filter(|&output| !pending.outputs.contains(output));
                new_outputs.extend(new);
                old_outputs.extend(old);
            }

            // If there is no current state, just apply all the pending outputs
            None => new_outputs.extend(pending.outputs.iter()),
        }

        for output in old_outputs {
            handle.output_leave(output);
        }

        for output in new_outputs {
            handle.output_enter(output);
        }

        if pending.parent.as_ref() != current.and_then(|h| h.parent.as_ref()) {
            if let Some(client) = handle.client() {
                let parent = pending
                    .parent
                    .as_ref()
                    .and_then(|p| p.handle_for_client(&client).ok());
                handle.parent(parent.as_ref());
            } else {
                // if looking up the client for the child failed, break the parent child relationship.
                handle.parent(None);
            }
        }

        handle.done();
    }

    fn init_state(&self, handle: &ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1) {
        if self.initialized.load(Ordering::Relaxed) {
            self.update_state(handle);
        }
    }
}

const VERSION: u32 = 1;

#[derive(Debug)]
struct ClientData {
    client: Client,
    // AtomicU32 is okay since the Wayland protocol forbids more than u32::MAX protocol objects to exist at once.
    toplevel_count: AtomicU32,
    stop: AtomicBool,
}

mod generated {
    #![allow(dead_code, non_camel_case_types, unused_unsafe, unused_variables)]
    #![allow(non_upper_case_globals, non_snake_case, unused_imports)]
    #![allow(missing_docs, clippy::all)]

    use wayland_server;
    use wayland_server::protocol::*;

    pub mod __interfaces {
        use wayland_server::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("ext-foreign-toplevel-info-v1.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_server_code!("ext-foreign-toplevel-info-v1.xml");
}

#[allow(missing_docs)]
pub mod protocol {
    pub use super::generated::*;
}
