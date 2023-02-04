use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use wayland_backend::server::{ClientId, ObjectId};
use wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource};

use super::{
    generated::{ext_foreign_toplevel_handle_v1, ext_foreign_toplevel_info_v1},
    ClientData, ForeignToplevelClient, ForeignToplevelInfo, ForeignToplevelInfoData,
    ForeignToplevelInfoHandler, GlobalData, ToplevelHandle, ToplevelHandleData,
};

impl<State> GlobalDispatch<ext_foreign_toplevel_info_v1::ExtForeignToplevelInfoV1, GlobalData, State>
    for ForeignToplevelInfo
where
    State: GlobalDispatch<ext_foreign_toplevel_info_v1::ExtForeignToplevelInfoV1, GlobalData, State>
        + Dispatch<ext_foreign_toplevel_info_v1::ExtForeignToplevelInfoV1, ForeignToplevelInfoData, State>
        + Dispatch<ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1, ToplevelHandleData, State>
        + ForeignToplevelInfoHandler
        + 'static,
{
    fn bind(
        state: &mut State,
        display: &DisplayHandle,
        client: &Client,
        new: New<ext_foreign_toplevel_info_v1::ExtForeignToplevelInfoV1>,
        _: &GlobalData,
        data_init: &mut DataInit<'_, State>,
    ) {
        let info = Arc::new(ClientData {
            client: client.clone(),
            handles: Mutex::new(Vec::new()),
            stop: AtomicBool::new(false),
        });

        let data = ForeignToplevelInfoData { info: info.clone() };
        let global = data_init.init(new, data);
        let toplevel_client = ForeignToplevelClient { object: global };

        state.foreign_toplevel().clients.push(toplevel_client.clone());
        state.new_client(toplevel_client.clone());

        // Quoting from the protocol:
        //
        // > If a compositor implementation sends the ext_foreign_toplevel_info_v1.finished
        // > event after the global is bound, the compositor must not send any
        // > ext_foreign_toplevel_info_v1.toplevel events.
        if toplevel_client.data().info.stop.load(Ordering::Relaxed) {
            return;
        }

        // Since a new client was created, send all toplevels to the new client.
        let state = state.foreign_toplevel();
        let mut new_handles = Vec::with_capacity(state.clients.len());

        // 1st loop, create all handles for the client. Is is necessary to avoid sending a child handle before
        // it's parent.
        for toplevel in state.toplevels.iter() {
            for client in state.clients.iter() {
                if let Some(handle) = toplevel.create_for_client::<State>(display, client) {
                    new_handles.push(handle);
                }
            }
        }

        // 2nd loop, send the current state of each toplevel to the new client.
        for handle in new_handles {
            let toplevel = ToplevelHandle::from_handle(&handle).unwrap();
            let state = toplevel.inner.state.lock().unwrap();

            if let Some(current) = state.current() {
                toplevel.inner.init_state(&handle, current);
            }
        }
    }

    fn can_view(client: Client, data: &GlobalData) -> bool {
        (data.filter)(&client)
    }
}

impl<State> Dispatch<ext_foreign_toplevel_info_v1::ExtForeignToplevelInfoV1, ForeignToplevelInfoData, State>
    for ForeignToplevelInfo
where
    State: Dispatch<ext_foreign_toplevel_info_v1::ExtForeignToplevelInfoV1, ForeignToplevelInfoData, State>
        + ForeignToplevelInfoHandler
        + 'static,
{
    fn request(
        _state: &mut State,
        _client: &Client,
        resource: &ext_foreign_toplevel_info_v1::ExtForeignToplevelInfoV1,
        request: ext_foreign_toplevel_info_v1::Request,
        data: &ForeignToplevelInfoData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, State>,
    ) {
        match request {
            ext_foreign_toplevel_info_v1::Request::Stop => {
                data.info.stop.store(true, Ordering::Release);
            }

            ext_foreign_toplevel_info_v1::Request::Destroy => {
                // Quoting from the protocol:
                //
                // > Destroying a ext_foreign_toplevel_info_v1 while there are toplevels still
                // > alive created by this ext_foreign_toplevel_info_v1 object is illegal and
                // > must result in a defunct_toplevels error.
                let handles = data.info.handles.lock().unwrap();

                if !handles.is_empty() {
                    resource.post_error(
                        ext_foreign_toplevel_info_v1::Error::DefunctToplevels,
                        "ext_foreign_toplevel_info_v1 was destroyed with defunct handles",
                    );
                }

                // Would be nice to use the more explicit Mutex::unlock, but that is not ready:
                // https://github.com/rust-lang/rust/issues/81872
                drop(handles);

                // client_destroyed will be called by the `destroyed` implementation, as that handles both
                // the global being destroyed and clients disconnecting.
            }

            #[allow(unreachable_patterns)] // in crate protocols cause non_exhaustive is ignored
            _ => unreachable!(),
        }
    }

    fn destroyed(
        state: &mut State,
        _client_id: ClientId,
        resource: ObjectId,
        _data: &ForeignToplevelInfoData,
    ) {
        // We can't get a ExtForeignToplevelInfoV1 using from_id here, but the existing
        // ExtForeignToplevelInfoV1 instances still share the same object id destroyed
        // gives us.

        // TODO: Replace this with drain_filter when stabilized.
        let mut destroyed_clients = Vec::new();
        state.foreign_toplevel().clients.retain(|client| {
            let destroy = client.info().id() != resource;

            if destroy {
                destroyed_clients.push(ForeignToplevelClient {
                    object: client.info().clone(),
                });
            }

            destroy
        });

        for destroyed in destroyed_clients {
            state.client_destroyed(&destroyed);
        }
    }
}

impl<State> Dispatch<ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1, ToplevelHandleData, State>
    for ForeignToplevelInfo
where
    State: Dispatch<ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1, ToplevelHandleData, State>
        + ForeignToplevelInfoHandler,
{
    fn request(
        _state: &mut State,
        _client: &Client,
        resource: &ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1,
        request: ext_foreign_toplevel_handle_v1::Request,
        data: &ToplevelHandleData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, State>,
    ) {
        match request {
            ext_foreign_toplevel_handle_v1::Request::Destroy => {
                let client_data = data.client.data();
                // We do not use Dispatch::destroyed here because that could be called when the client
                // has disconnected. Plus our error checking for defunct handles only matters for live
                // clients.
                let mut handles = client_data.info.handles.lock().unwrap();
                handles.retain(|entry| entry != resource);
            }

            #[allow(unreachable_patterns)] // in crate non_exhaustive is ignored
            _ => unreachable!(),
        }
    }
}
