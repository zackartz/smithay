use std::sync::Mutex;

use slog::debug;
use wayland_server::{
    protocol::{wl_data_device, wl_data_device_manager, wl_seat, wl_data_source},
    Client, DataInit, DelegateDispatch, DelegateDispatchBase, Dispatch, DisplayHandle, Resource,
};

use crate::wayland::{
    compositor,
    data_device::{dnd_grab, DND_ICON_ROLE, DataDeviceEvent, SourceMetadata},
    seat::SeatHandler,
    Serial,
};

use super::{DataDeviceHandler, DataDeviceState, data_source::DataSourceData};

impl DelegateDispatchBase<wl_data_device_manager::WlDataDeviceManager> for DataDeviceState {
    type UserData = ();
}

impl<D> DelegateDispatch<wl_data_device_manager::WlDataDeviceManager, D> for DataDeviceState
where
    D: Dispatch<wl_data_device_manager::WlDataDeviceManager, UserData = Self::UserData>
        + DataDeviceHandler
        + SeatHandler<D>,
{
    fn request(
        state: &mut D,
        client: &Client,
        resource: &wl_data_device_manager::WlDataDeviceManager,
        request: wl_data_device_manager::Request,
        data: &Self::UserData,
        dhandle: &mut DisplayHandle<'_>,
        data_init: &mut DataInit<'_, D>,
    ) {
        use self::wl_data_device_manager::Request;

        match request {
            Request::CreateDataSource { id } => {
                let data = DataSourceData {
                    meta: Mutex::new(SourceMetadata {
                        mime_types: vec![],
                        dnd_action: wl_data_device_manager::DndAction::None,
                    }),
                };

                data_init.init(id, data);
            }

            Request::GetDataDevice { id, seat } => {
                data_init.init(id, seat);
            }

            _ => unreachable!(),
        }
    }
}

impl DelegateDispatchBase<wl_data_source::WlDataSource> for DataDeviceState {
    type UserData = DataSourceData;
}

impl<D> DelegateDispatch<wl_data_source::WlDataSource, D> for DataDeviceState
where
    D: Dispatch<wl_data_source::WlDataSource, UserData = Self::UserData>,
{
    fn request(
        state: &mut D,
        client: &Client,
        resource: &wl_data_source::WlDataSource,
        request: wl_data_source::Request,
        data: &Self::UserData,
        dhandle: &mut DisplayHandle<'_>,
        data_init: &mut DataInit<'_, D>,
    ) {
        use self::wl_data_source::Request;

        match request {
            Request::Offer { mime_type } => {
                let guard = data.meta.lock().unwrap();
                guard.mime_types.push(mime_type);
            },

            Request::SetActions { dnd_actions } => {
                let mut guard = data.meta.lock().unwrap();
                guard.dnd_action = wl_data_device_manager::DndAction::from_bits_truncate(dnd_actions.into());
            },

            Request::Destroy => {},

            _ => unreachable!()
        }
    }
}

impl DelegateDispatchBase<wl_data_device::WlDataDevice> for DataDeviceState {
    type UserData = wl_seat::WlSeat;
}

impl<D> DelegateDispatch<wl_data_device::WlDataDevice, D> for DataDeviceState
where
    D: Dispatch<wl_data_device::WlDataDevice, UserData = ()> + DataDeviceHandler + SeatHandler<D>,
{
    fn request(
        state: &mut D,
        client: &Client,
        resource: &wl_data_device::WlDataDevice,
        request: wl_data_device::Request,
        data: &Self::UserData,
        dh: &mut DisplayHandle<'_>,
        data_init: &mut DataInit<'_, D>,
    ) {
        use self::wl_data_device::Request;

        match request {
            Request::StartDrag {
                source,
                origin,
                icon,
                serial,
            } => {
                let serial = Serial::from(serial);

                if let Some(pointer) = state.seat_state().get_pointer() {
                    if pointer.has_grab(serial) {
                        if let Some(ref icon) = icon {
                            if compositor::give_role(icon, DND_ICON_ROLE).is_err() {
                                resource.post_error(
                                    dh,
                                    wl_data_device::Error::Role,
                                    "given surface already has another role.",
                                );
                                return;
                            }

                            // The StartDrag is in response to a pointer implicit grab, which is fine.
                            state.event(DataDeviceEvent::DnDStarted {
                                source: source.clone(),
                                icon: icon.clone(),
                                seat: data.clone(),
                            });
                            let start_data = pointer.grab_start_data().unwrap();

                            let grab = dnd_grab::DnDGrab::new(
                                start_data,
                                source,
                                origin,
                                data.clone(),
                                icon,
                                todo!("callback not needed"),
                            );

                            pointer.set_grab(dh, grab, serial, 0);
                        }
                    }
                } else {
                    // TODO
                    //debug!(log, "denying drag from client without implicit grab");
                }
            }

            Request::SetSelection { source, serial } => {
                if let Some(keyboard) = state.seat_state().get_keyboard() {
                    if keyboard.has_focus(client) {
                        state.event(DataDeviceEvent::NewSelection(source.clone()));
                        // The client has kbd focus, it can set the selection.

                        // TODO: Set selection on seat
                    } else {
                        // TODO
                        //debug!(log, "denying setting selection by a non-focused client");
                    }
                }
            },

            Request::Release => {
                // TODO: Clean up known devices
                todo!()
            },

            _ => unreachable!(),
        }
    }
}
