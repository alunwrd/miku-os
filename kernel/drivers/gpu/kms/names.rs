// Linux's names for connectors
//
// A `monitor = DP-3, ...` line written on Linux means the same socket here. DRM
// names a connector by its type and a counter per type, from 1, in creation
// order (`drm_connector_init`). nvidia-drm creates them walking the display ids
// in ascending order (`GetDisplays`, `nvkms-kapi.c`), and makes one connector
// per physical socket (`nv_drm_get_connector`, `nvidia-drm-connector.c:586`): a
// DP++ port answers to a DP id and a TMDS id, both encoders on one connector
// named after the first. On this card seven display ids become `HDMI-A-1` and
// `DP-1`..`DP-3` (`modetest -M nvidia-drm -c`).
//
// Pure logic, so the selftest decides it with no card.

use alloc::string::String;
use alloc::vec::Vec;

use super::Connector;

/// One display id, and the socket and kind its driver says it is
#[derive(Clone, Copy)]
pub struct Face {
    pub key: u32,
    pub physical: u32,
    pub connector: Connector,
}

/// A connector: its name and every display id that is a face of it
pub struct Named {
    pub name: String,
    pub connector: Connector,
    pub physical: u32,
    /// Ascending, the first being the one it was named after
    pub keys: Vec<u32>,
}

/// Group `faces`, ascending by key, into connectors and name them.
///
/// A face whose socket is already a connector joins it with whatever kind it
/// claims: nvidia-drm requires the kinds to agree (`BUG_ON`), and the first
/// face's is the one the connector was created with
pub fn name(faces: &[Face]) -> Vec<Named> {
    let mut out: Vec<Named> = Vec::new();
    for f in faces {
        if let Some(n) = out.iter_mut().find(|n| n.physical == f.physical) {
            n.keys.push(f.key);
            continue;
        }
        let kind = f.connector.drm_name();
        let nth = out.iter().filter(|n| n.connector.drm_name() == kind).count() + 1;
        out.push(Named {
            name: alloc::format!("{}-{}", kind, nth),
            connector: f.connector,
            physical: f.physical,
            keys: alloc::vec![f.key],
        });
    }
    out
}
