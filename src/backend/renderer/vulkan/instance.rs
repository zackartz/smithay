use std::{
    ffi::{CStr, CString, NulError},
    fmt::{self, Formatter},
    sync::Arc,
};

use ash::{
    vk::{self, ExtendsInstanceCreateInfo},
    Entry,
};

lazy_static::lazy_static! {
    pub(crate) static ref LIBRARY: Entry = Entry::new();
}

/// An error that may occur when creating an [`Instance`].
#[derive(Debug, thiserror::Error)]
pub enum InstanceError {
    /// Some extensions were missing when creating an instance.
    #[error("The following extensions instance extensions were missing: {}", .0.join(", "))]
    MissingExtensions(Vec<String>),

    /// An extension has an invalid name.
    #[error("The extension string \"{0}\" contains a null terminator at {}", .1.nul_position())]
    InvalidExtension(String, NulError),

    /// Some layers were missing when creating an instance.
    #[error("The following extensions instance layers were missing: {}", .0.join(", "))]
    MissingLayers(Vec<String>),

    /// A layer has an invalid name.
    #[error("The layer string \"{0}\" contains a null terminator at {}", .1.nul_position())]
    InvalidLayer(String, NulError),

    /// Some Vulkan error occurred while creating an instance.
    #[error("{0}")]
    Vulkan(#[from] vk::Result),
}

/// A Vulkan instance.
///
/// TODO: Docs
#[derive(Debug)]
pub struct Instance {
    pub(crate) inner: Arc<InstanceInner>,
}

impl Instance {
    /// Returns an iterator containing all extensions a Vulkan instance may be created with.
    pub fn enumerate_extensions() -> Result<impl Iterator<Item = String>, InstanceError> {
        let extensions = LIBRARY
            .enumerate_instance_extension_properties()?
            .into_iter()
            .map(|extension|
                // SAFETY: Vulkan always returns null terminated strings with a maximum length of 256
                unsafe { CStr::from_ptr(extension.extension_name.as_ptr()) })
            .map(|s| {
                s.to_str()
                    .expect("Vulkan reported a non-UTF8 extension name")
                    .to_owned()
            })
            .collect::<Vec<_>>()
            .into_iter();

        Ok(extensions)
    }

    /// Returns an iterator containing all layers a Vulkan instance may be created with.
    pub fn enumerate_layers() -> Result<impl Iterator<Item = String>, InstanceError> {
        let layers = LIBRARY
            .enumerate_instance_layer_properties()?
            .into_iter()
            .map(|layer|
                // SAFETY: Vulkan always returns null terminated strings with a maximum length of 256
                unsafe { CStr::from_ptr(layer.layer_name.as_ptr()) })
            .map(|s| {
                s.to_str()
                    .expect("Vulkan reported a non-UTF8 layer name")
                    .to_owned()
            })
            .collect::<Vec<_>>()
            .into_iter();

        Ok(layers)
    }

    /// Creates a new Vulkan instance.
    ///
    /// This function takes a list of required extensions.
    pub fn with_extensions<'ext, E>(extensions: E) -> Result<Instance, InstanceError>
    where
        E: IntoIterator<Item = &'ext str>,
    {
        Self::with_extensions_and_layers(extensions, std::iter::empty())
    }

    /// Creates a new Vulkan instance.
    ///
    /// This function takes two parameters. A list of required extensions and a list of required layers.
    pub fn with_extensions_and_layers<'a, E, L>(extensions: E, layers: L) -> Result<Instance, InstanceError>
    where
        E: IntoIterator<Item = &'a str>,
        L: IntoIterator<Item = &'a str>,
    {
        let (extensions, layers) = Self::validate_extensions_and_layers(extensions, layers)?;

        let extensions = &extensions
            .into_iter()
            .map(|extension| extension.as_ptr())
            .collect::<Vec<_>>();

        let layers = &layers.iter().map(|layer| layer.as_ptr()).collect::<Vec<_>>();

        let app_info = vk::ApplicationInfo::builder().api_version(vk::API_VERSION_1_1); // TODO: Allow configuring version?

        let builder = vk::InstanceCreateInfo::builder()
            .application_info(&app_info)
            .enabled_extension_names(extensions)
            .enabled_layer_names(layers);

        // SAFETY: CString ensures all extension and layer names are UTF-8 null terminated strings that are safe for Vulkan.
        let handle = unsafe { LIBRARY.create_instance(&builder, None) }?;

        Ok(Instance {
            inner: unsafe { InstanceInner::new(handle) }, // SAFETY: We own the instance.
        })
    }

    /// Creates a new Vulkan instance.
    ///
    /// This function takes two parameters. A list of required extensions and a list of required layers.
    ///
    /// This function also takes a chain of objects which implement [`ExtendsInstanceCreateInfo`] may be used to create an instance with some
    /// additional validation and debugging features beyond regular validation layers.
    ///
    /// # Safety
    ///
    /// Extension values passed in to create the instance must be valid per the Vulkan specification.
    pub unsafe fn with_extensions_and_layers_and_chain<'a, E, L, C>(
        extensions: E,
        layers: L,
        chain: &'a mut C,
    ) -> Result<Instance, InstanceError>
    where
        E: IntoIterator<Item = &'a str>,
        L: IntoIterator<Item = &'a str>,
        C: ExtendsInstanceCreateInfo,
    {
        let (extensions, layers) = Self::validate_extensions_and_layers(extensions, layers)?;

        let extensions = &extensions
            .into_iter()
            .map(|extension| extension.as_ptr())
            .collect::<Vec<_>>();

        let layers = &layers.iter().map(|layer| layer.as_ptr()).collect::<Vec<_>>();

        let app_info = vk::ApplicationInfo::builder().api_version(vk::API_VERSION_1_1); // TODO: Allow configuring version?

        let builder = vk::InstanceCreateInfo::builder()
            .application_info(&app_info)
            .enabled_extension_names(extensions)
            .enabled_layer_names(layers)
            .push_next(chain);

        // SAFETY: CString ensures all extension and layer names are UTF-8 null terminated strings that are safe for Vulkan.
        let handle = LIBRARY.create_instance(&builder, None)?;

        Ok(Instance {
            inner: InstanceInner::new(handle), // SAFETY: We own the instance.
        })
    }

    /// Returns a handle to a ash's Vulkan instance type.
    ///
    /// The handle may be used to get access to all the core and extension functions available on an Instance.
    ///
    /// ## Safety
    ///
    /// The caller responsible for ensuring the returned handle is only used while the instance are valid.
    ///
    /// The caller is also responsible for ensuring any child objects created by the instance are destroyed
    /// before the instance is dropped.
    pub unsafe fn raw_handle(&self) -> &ash::Instance {
        &self.inner.instance
    }

    fn validate_extensions_and_layers<'a, E, L>(
        extensions: E,
        layers: L,
    ) -> Result<(Vec<CString>, Vec<CString>), InstanceError>
    where
        E: IntoIterator<Item = &'a str>,
        L: IntoIterator<Item = &'a str>,
    {
        let extensions = {
            let extensions = extensions.into_iter();
            let extensions_count = {
                let size_hint = extensions.size_hint();

                size_hint.1.unwrap_or(size_hint.0)
            };
            let mut c_extensions = Vec::with_capacity(extensions_count);

            for extension in extensions {
                c_extensions.push(
                    CString::new(extension)
                        .map_err(|err| InstanceError::InvalidExtension(extension.to_owned(), err))?,
                );
            }

            c_extensions
        };

        let layers = {
            let layers = layers.into_iter();
            let layer_count = {
                let size_hint = layers.size_hint();

                size_hint.1.unwrap_or(size_hint.0)
            };
            let mut c_layers = Vec::with_capacity(layer_count);

            for layer in layers {
                c_layers.push(
                    CString::new(layer).map_err(|err| InstanceError::InvalidLayer(layer.to_owned(), err))?,
                );
            }

            c_layers
        };

        Ok((extensions, layers))
    }

    pub(crate) fn impl_clone(&self) -> Instance {
        Instance {
            inner: self.inner.clone(),
        }
    }
}

/// The inner instance.
///
/// This type is a container for
pub(crate) struct InstanceInner {
    instance: ash::Instance,
}

impl InstanceInner {
    /// Creates a new instance inner from an instance handle.
    ///
    /// # Safety
    ///
    /// The caller must pass ownership of the instance to the instance inner.
    unsafe fn new(instance: ash::Instance) -> Arc<InstanceInner> {
        Arc::new(InstanceInner { instance })
    }
}

impl fmt::Debug for InstanceInner {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("InstanceInner")
            .field("instance", &self.instance.handle())
            .finish()
    }
}

impl Drop for InstanceInner {
    fn drop(&mut self) {
        // SAFETY (Synchronization):
        // - Externally synchronized because InstanceInner is always inside an `Arc`.
        // - Access to all VkPhysicalDevices are synchronized because Smithay's `PhysicalDevice` has a
        //   lifetime on the `Instance`, therefore meaning all physical devices must be dropped before
        //   dropping an instance is possible.
        // SAFETY (Usage):
        // - All child objects of the instance hold strong references to the `Instance`, therefore all child
        //   objects will be dropped before the instance is.
        unsafe {
            self.instance.destroy_instance(None);
        }
    }
}
