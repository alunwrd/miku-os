// Chip-independent GSP-RM plumbing (post-boot RPC world)
//
// Once GSP-RM is running - whether it got there via the Turing
// booter/SEC2 chain or the Blackwell FSP/COT chain - the host talks to it
// through the same CMDQ/MSGQ shared region and the same RPC framing.
// This module holds that shared layer; the per-chip drivers only differ
// in how they boot the RISC-V core

pub mod rpc;
pub mod sysinfo;
