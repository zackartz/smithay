use std::sync::{
    atomic::{AtomicBool, AtomicU32, Ordering},
    Arc,
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
        // Clients are only allowed to bind one global at a time.
        if state.foreign_toplevel().clients.contains_key(&client.id()) {
            // TODO: wayland-rs has no way to make a New post an error without a panic.
            // data_init.init(new, todo!()).post_error(
            //     ext_foreign_toplevel_info_v1::Error::AlreadyConstructed,
            //     "The ext_foreign_toplevel_info_v1 global has already been instantiated",
            // );
            return;
        }

        let info = Arc::new(ClientData {
            client: client.clone(),
            toplevel_count: AtomicU32::new(0),
            stop: AtomicBool::new(false),
        });

        let data = ForeignToplevelInfoData { info: info.clone() };
        let global = data_init.init(new, data);
        let toplevel_client = ForeignToplevelClient { object: global };

        state
            .foreign_toplevel()
            .clients
            .insert(client.id(), toplevel_client.clone());
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
            for client in state.clients.values() {
                if let Some(handle) = toplevel.create_for_client::<State>(display, client) {
                    new_handles.push(handle);
                }
            }
        }

        // 2nd loop, send the current state of each toplevel to the new client.
        for handle in new_handles {
            let toplevel = ToplevelHandle::from_handle(&handle).unwrap();
            toplevel.inner.init_state(&handle);
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
                if data.info.toplevel_count.load(Ordering::Acquire) > 0 {
                    resource.post_error(
                        ext_foreign_toplevel_info_v1::Error::DefunctToplevels,
                        "ext_foreign_toplevel_info_v1 was destroyed with defunct handles",
                    );
                }

                // client_destroyed will be called by the `destroyed` implementation.
            }

            #[allow(unreachable_patterns)] // in tree protocols cause non_exhaustive is ignored
            _ => unreachable!(),
        }
    }

    fn destroyed(
        state: &mut State,
        client_id: ClientId,
        resource: ObjectId,
        _data: &ForeignToplevelInfoData,
    ) {
        let display = state.foreign_toplevel().display.clone();
        let client = ForeignToplevelClient {
            object: ext_foreign_toplevel_info_v1::ExtForeignToplevelInfoV1::from_id(&display, resource)
                .unwrap(),
        };
        state.client_destroyed(&client);
        state.foreign_toplevel().clients.remove(&client_id);
    }
}

impl<State> Dispatch<ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1, ToplevelHandleData, State>
    for ForeignToplevelInfo
where
    State: Dispatch<ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1, ToplevelHandleData, State>
        + ForeignToplevelInfoHandler,
{
    fn request(
        state: &mut State,
        client: &Client,
        _resource: &ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1,
        request: ext_foreign_toplevel_handle_v1::Request,
        _data: &ToplevelHandleData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, State>,
    ) {
        match request {
            ext_foreign_toplevel_handle_v1::Request::Destroy => {
                if let Some(data) = state.foreign_toplevel().clients.get(&client.id()) {
                    data.data().info.toplevel_count.fetch_sub(1, Ordering::Relaxed);
                }
                // destroyed handles all cleanup
            }
            #[allow(unreachable_patterns)] // in tree protocols cause non_exhaustive is ignored
            _ => unreachable!(),
        }
    }

    fn destroyed(_state: &mut State, _client: ClientId, resource: ObjectId, data: &ToplevelHandleData) {
        let mut handles = data.inner.handles.lock().unwrap();
        handles.retain(|handle| handle.id() != resource);
    }
}
