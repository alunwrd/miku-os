// drivers/block - storage device drivers (lower edge of the block layer)
// Each implements the BlockDriver trait from 'crate::io::block::driver'

pub mod ahci;
pub mod ata;
pub mod nvme;
pub mod ramdisk;
pub mod virtio_blk;
