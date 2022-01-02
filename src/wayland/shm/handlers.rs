use super::{
    pool::{Pool, ResizeError},
    ShmDispatch,
};

use std::sync::Arc;
use wayland_server::{
    protocol::{
        wl_buffer::{self, WlBuffer},
        wl_shm::{self, WlShm},
        wl_shm_pool::{self, WlShmPool},
    },
    DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
};

use crate::wayland::{
    delegate::{DelegateDispatch, DelegateDispatchBase, DelegateGlobalDispatch, DelegateGlobalDispatchBase},
    shm::BufferData,
};

/*
 * wl_shm
 */

impl DelegateGlobalDispatchBase<WlShm> for ShmDispatch<'_> {
    type GlobalData = ();
}

impl<D: 'static> DelegateGlobalDispatch<WlShm, D> for ShmDispatch<'_>
where
    D: GlobalDispatch<WlShm, GlobalData = ()>
        + Dispatch<WlShm, UserData = ()>
        + Dispatch<WlShmPool, UserData = ShmPoolUserData>,
{
    fn bind(
        &mut self,
        handle: &mut wayland_server::DisplayHandle<'_, D>,
        _client: &wayland_server::Client,
        resource: New<WlShm>,
        _global_data: &Self::GlobalData,
        data_init: &mut DataInit<'_, D>,
    ) {
        let shm = data_init.init(resource, ());

        // send the formats
        for &f in &self.0.formats[..] {
            shm.format(handle, f);
        }
    }
}

impl DelegateDispatchBase<WlShm> for ShmDispatch<'_> {
    type UserData = ();
}

impl<D: 'static> DelegateDispatch<WlShm, D> for ShmDispatch<'_>
where
    D: Dispatch<WlShm, UserData = ()> + Dispatch<WlShmPool, UserData = ShmPoolUserData>,
{
    fn request(
        &mut self,
        _client: &wayland_server::Client,
        shm: &WlShm,
        request: wl_shm::Request,
        _data: &Self::UserData,
        cx: &mut DisplayHandle<'_, D>,
        data_init: &mut DataInit<'_, D>,
    ) {
        use wl_shm::{Error, Request};

        let (pool, fd, size) = match request {
            Request::CreatePool { id: pool, fd, size } => (pool, fd, size),
            _ => unreachable!(),
        };
        if size <= 0 {
            shm.post_error(cx, Error::InvalidFd, "Invalid size for a new wl_shm_pool.");
            return;
        }
        let mmap_pool = match Pool::new(fd, size as usize, self.0.log.clone()) {
            Ok(p) => p,
            Err(()) => {
                shm.post_error(cx, wl_shm::Error::InvalidFd, format!("Failed mmap of fd {}.", fd));
                return;
            }
        };

        data_init.init(
            pool,
            ShmPoolUserData {
                inner: Arc::new(mmap_pool),
            },
        );
    }
}

/*
 * wl_shm_pool
 */

/// User data of WlShmPool
#[derive(Debug)]
pub struct ShmPoolUserData {
    inner: Arc<Pool>,
}

impl DelegateDispatchBase<WlShmPool> for ShmDispatch<'_> {
    type UserData = ShmPoolUserData;
}

impl<D: 'static> DelegateDispatch<WlShmPool, D> for ShmDispatch<'_>
where
    D: Dispatch<WlShmPool, UserData = ShmPoolUserData> + Dispatch<WlBuffer, UserData = ShmBufferUserData>,
{
    fn request(
        &mut self,
        _client: &wayland_server::Client,
        pool: &WlShmPool,
        request: wl_shm_pool::Request,
        data: &Self::UserData,
        cx: &mut DisplayHandle<'_, D>,
        data_init: &mut DataInit<'_, D>,
    ) {
        use self::wl_shm_pool::Request;

        let arc_pool = &data.inner;

        match request {
            Request::CreateBuffer {
                id: buffer,
                offset,
                width,
                height,
                stride,
                format,
            } => {
                // Validate client parameters
                let message = if offset < 0 {
                    Some("offset must not be negative".to_string())
                } else if width <= 0 || height <= 0 {
                    Some(format!("invalid width or height ({}x{})", width, height))
                } else if stride < width {
                    Some(format!(
                        "width must not be larger than stride (width {}, stride {})",
                        width, stride
                    ))
                } else if (i32::MAX / stride) < height {
                    Some(format!(
                        "height is too large for stride (max {})",
                        i32::MAX / stride
                    ))
                } else if offset > arc_pool.size() as i32 - (stride * height) {
                    Some("offset is too large".to_string())
                } else {
                    None
                };

                if let Some(message) = message {
                    pool.post_error(cx, wl_shm::Error::InvalidStride, message);
                    return;
                }

                if let WEnum::Value(format) = format {
                    if !self.0.formats.contains(&format) {
                        pool.post_error(
                            cx,
                            wl_shm::Error::InvalidFormat,
                            format!("SHM format {:?} is not supported.", format),
                        );
                        return;
                    }
                    let data = ShmBufferUserData {
                        pool: arc_pool.clone(),
                        data: BufferData {
                            offset,
                            width,
                            height,
                            stride,
                            format,
                        },
                    };

                    data_init.init(buffer, data);
                }
            }
            Request::Resize { size } => match arc_pool.resize(size) {
                Ok(()) => {}
                Err(ResizeError::InvalidSize) => {
                    pool.post_error(
                        cx,
                        wl_shm::Error::InvalidFd,
                        "Invalid new size for a wl_shm_pool.",
                    );
                }
                Err(ResizeError::MremapFailed) => {
                    pool.post_error(cx, wl_shm::Error::InvalidFd, "mremap failed.");
                }
            },
            Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}

/*
 * wl_buffer
 */

/// User data of shm WlBuffer
#[derive(Debug)]
pub struct ShmBufferUserData {
    pub(crate) pool: Arc<Pool>,
    pub(crate) data: BufferData,
}

impl DelegateDispatchBase<WlBuffer> for ShmDispatch<'_> {
    type UserData = ShmBufferUserData;
}

impl<D: 'static> DelegateDispatch<WlBuffer, D> for ShmDispatch<'_>
where
    D: Dispatch<WlBuffer, UserData = ShmBufferUserData>,
{
    fn request(
        &mut self,
        _client: &wayland_server::Client,
        _pool: &WlBuffer,
        _request: wl_buffer::Request,
        _data: &Self::UserData,
        _cx: &mut DisplayHandle<'_, D>,
        _data_init: &mut DataInit<'_, D>,
    ) {
    }
}
