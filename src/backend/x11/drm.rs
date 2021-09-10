//! Utilities for checking properties of a drm device.

/*
About certain this needs checking
*/

use std::os::unix::prelude::RawFd;

use nix::{
    errno::Errno,
    sys::stat::{fstat, major, minor, SFlag},
};

/// This function is a copy of `drmGetNodeTypeFromFd` from libdrm.
pub fn get_drm_node_type(fd: RawFd) -> Result<u64, Errno> {
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
        todo!()
    }

    Ok(drm_get_minor_type(major, minor).expect("TODO"))
}

// These are actually in use.
#[allow(dead_code)]
pub const DRM_NODE_PRIMARY: u64 = 0;
#[allow(dead_code)]
pub const DRM_NODE_CONTROL: u64 = 1;
#[allow(dead_code)]
pub const DRM_NODE_RENDER: u64 = 2;

pub fn drm_get_minor_type(_major: u64, minor: u64) -> Result<u64, ()> {
    #[cfg(target_os = "freebsd")]
    compile_error!("FreeBSD is not implemented yet!");

    // TODO: What on earth is libdrm doing here with bit magic.
    // the stat might already hold this information?
    let ty = minor >> 6;

    match ty {
        DRM_NODE_PRIMARY | DRM_NODE_CONTROL | DRM_NODE_RENDER => Ok(ty),
        _ => Err(()),
    }
}

// drmNodeIsDRM has differing implementations on each os

#[cfg(target_os = "linux")]
pub fn is_drm_node_drm(major: u64, minor: u64) -> bool {
    use nix::sys::stat::stat;

    let path = format!("/sys/dev/char/{}:{}/device/drm", major, minor);
    // drmNodeIsDRM under the Linux preprocessor line seems to limit the length of the path to 64 characters including terminator.
    assert!(path.len() <= 63);
    stat(path.as_str()).is_ok()
}

#[cfg(target_os = "freebsd")]
pub fn is_drm_node_drm(major: u64, minor: u64) -> bool {
    compile_error!("FreeBSD not implemented yet!")
}

#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
pub fn is_drm_node_drm(major: u64, minor: u64) -> bool {
    compile_error!("Non Linux and FreeBSD not implemented yet!")
}
