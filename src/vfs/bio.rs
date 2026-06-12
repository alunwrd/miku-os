use crate::vfs::types::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BioDirection {
    Read = 0,
    Write = 1,
    /// TRIM/deallocate - carries no data, only a sector range
    Discard = 2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BioStatus {
    Pending = 0,
    InProgress = 1,
    Complete = 2,
    Error = 3,
}

#[derive(Clone, Copy)]
pub struct BioRequest {
    pub direction: BioDirection,
    pub status: BioStatus,
    pub device_id: BlockDevId,
    pub block_num: u64,
    pub block_count: u16,
    pub buffer_page: PageId,
    pub active: bool,
}

impl BioRequest {
    pub const fn empty() -> Self {
        Self {
            direction: BioDirection::Read,
            status: BioStatus::Pending,
            device_id: INVALID_U8,
            block_num: 0,
            block_count: 0,
            buffer_page: INVALID_ID,
            active: false,
        }
    }
}

pub struct BioQueue {
    pub requests: [BioRequest; MAX_BIO_QUEUE],
    pub total_submitted: u64,
    pub total_completed: u64,
    pub total_errors: u64,
}

impl BioQueue {
    pub const fn new() -> Self {
        Self {
            requests: [BioRequest::empty(); MAX_BIO_QUEUE],
            total_submitted: 0,
            total_completed: 0,
            total_errors: 0,
        }
    }

    pub fn submit(
        &mut self,
        direction: BioDirection,
        device_id: BlockDevId,
        block_num: u64,
        block_count: u16,
        buffer_page: PageId,
    ) -> VfsResult<usize> {
        for (i, req) in self.requests.iter_mut().enumerate() {
            if !req.active {
                *req = BioRequest {
                    direction,
                    status: BioStatus::Pending,
                    device_id,
                    block_num,
                    block_count,
                    buffer_page,
                    active: true,
                };
                self.total_submitted += 1;
                return Ok(i);
            }
        }
        Err(VfsError::NoSpace)
    }

    pub fn complete(&mut self, idx: usize, success: bool) {
        if idx < MAX_BIO_QUEUE && self.requests[idx].active {
            if success {
                self.requests[idx].status = BioStatus::Complete;
                self.total_completed += 1;
            } else {
                self.requests[idx].status = BioStatus::Error;
                self.total_errors += 1;
            }
            self.requests[idx].active = false;
        }
    }

    pub fn pending_count(&self) -> usize {
        self.requests
            .iter()
            .filter(|r| r.active && r.status == BioStatus::Pending)
            .count()
    }
}
