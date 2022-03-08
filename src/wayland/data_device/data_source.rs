use std::{cell::RefCell, ops::Deref as _, sync::Mutex};

use wayland_server::{
    protocol::{
        wl_data_device_manager::DndAction,
        wl_data_source::{Request, WlDataSource},
    },
};

#[derive(Debug)]
pub struct DataSourceData {
    pub(super) meta: Mutex<SourceMetadata>,
}

/// The metadata describing a data source
#[derive(Debug, Clone)]
pub struct SourceMetadata {
    /// The MIME types supported by this source
    pub mime_types: Vec<String>,
    /// The Drag'n'Drop actions supported by this source
    pub dnd_action: DndAction,
}

/// Access the metadata of a data source
pub fn with_source_metadata<T, F: FnOnce(&SourceMetadata) -> T>(
    source: &WlDataSource,
    f: F,
) -> Result<T, crate::utils::UnmanagedResource> {
    match source.as_ref().user_data().get::<RefCell<SourceMetadata>>() {
        Some(data) => Ok(f(&data.borrow())),
        None => Err(crate::utils::UnmanagedResource),
    }
}
