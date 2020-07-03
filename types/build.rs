#[cfg(not(feature = "no-unstable-features"))]
use rustc_version::{version_meta, Channel, VersionMeta};

fn main() {
    #[cfg(not(feature = "no-unstable-features"))]
    if let Ok(VersionMeta { channel: Channel::Stable, .. }) = version_meta() {
        println!("cargo:rustc-cfg=feature=\"no-unstable-features\"");
   }
}