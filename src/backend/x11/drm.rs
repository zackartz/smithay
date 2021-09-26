//! Utilities for checking properties of a drm device.

/*
About certain this needs checking
*/

use std::{
    error::Error,
    fmt::{self, Display, Formatter},
    os::unix::prelude::RawFd,
};

use nix::sys::stat::{fstat, major, minor, SFlag};

use super::AllocateBuffersError;

/// This function is a copy of `drmGetNodeTypeFromFd` from libdrm.
pub fn get_drm_node_type_from_fd(fd: RawFd) -> Result<u64, AllocateBuffersError> {
    // Obtain major and minor numbers of the file descriptor
    let stat_buf = fstat(fd)?;

    let major = major(stat_buf.st_rdev);
    let minor = minor(stat_buf.st_rdev);

    let stat_flags = SFlag::from_bits_truncate(stat_buf.st_mode);

    // isDrm
    if !is_drm_node_drm(major, minor)
        // Extract file type code with S_IFMT
        //
        // Then check if we have a character device by seeing if the leftover is equal to S_IFCHR
        || (stat_flags & SFlag::S_IFMT) != SFlag::S_IFCHR
    {
        return Err(AllocateBuffersError::UnsupportedDrmNode);
    }

    drm_get_minor_type(major, minor)
}

// These are actually in use.
#[allow(dead_code)]
pub const DRM_NODE_PRIMARY: u64 = 0;
#[allow(dead_code)]
pub const DRM_NODE_CONTROL: u64 = 1;
#[allow(dead_code)]
pub const DRM_NODE_RENDER: u64 = 2;

// Again copied from xf86drm.c

#[cfg(target_os = "dragonfly")]
pub const DRM_MAJOR: u64 = 145;

#[cfg(target_os = "netbsd")]
pub const DRM_MAJOR: u64 = 34;

#[cfg(all(target_os = "openbsd", target_arch = "i386"))]
pub const DRM_MAJOR: u64 = 88;

#[cfg(all(target_os = "openbsd", not(target_arch = "i386")))]
pub const DRM_MAJOR: u64 = 87;

// libdrm uses the Linux value as the fallback where a DRM_MAJOR isn't otherwise defined.
#[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
#[allow(dead_code)]
pub const DRM_MAJOR: u64 = 226;

#[derive(Debug)]
pub struct UnsupportedDrmNodeType;

impl Display for UnsupportedDrmNodeType {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "the drm node type is not supported")
    }
}

impl Error for UnsupportedDrmNodeType {}

/// This function is a copy of `drmGetMinorType` from libdrm
pub fn drm_get_minor_type(_major: u64, minor: u64) -> Result<u64, AllocateBuffersError> {
    #[cfg(target_os = "freebsd")]
    compile_error!("FreeBSD is not implemented yet!");

    // TODO: What on earth is libdrm doing here with bit magic.
    // the stat might already hold this information?
    let ty = minor >> 6;

    match ty {
        DRM_NODE_PRIMARY | DRM_NODE_CONTROL | DRM_NODE_RENDER => Ok(ty),
        _ => Err(AllocateBuffersError::UnsupportedDrmNode),
    }
}

// drmNodeIsDRM has differing implementations on each os

/// This function is a copy of `isDrmNodeDrm` from libdrm
#[cfg(target_os = "linux")]
pub fn is_drm_node_drm(major: u64, minor: u64) -> bool {
    use nix::sys::stat::stat;

    let path = format!("/sys/dev/char/{}:{}/device/drm", major, minor);
    // drmNodeIsDRM under the Linux preprocessor line seems to limit the length of the path to 64 characters including terminator.
    assert!(path.len() <= 63);
    stat(path.as_str()).is_ok()
}

/// This function is a copy of `isDrmNodeDrm` from libdrm
#[cfg(target_os = "freebsd")]
pub fn is_drm_node_drm(major: u64, minor: u64) -> bool {
    compile_error!("FreeBSD not implemented yet!")
}

/// This function is a copy of `isDrmNodeDrm` from libdrm
#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
pub fn is_drm_node_drm(major: u64, _minor: u64) -> bool {
    major == DRM_MAJOR
}
