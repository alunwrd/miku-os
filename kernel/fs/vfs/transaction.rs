use crate::fs::vfs::types::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TxState {
    Idle = 0,
    Active = 1,
    Committed = 2,
    Aborted = 3,
}

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum TxOpKind {
    Create = 1,
    Delete = 2,
    Write = 3,
    Rename = 4,
    SetAttr = 5,
}

#[derive(Clone, Copy)]
pub struct TxOp {
    pub kind: TxOpKind,
    pub vnode_id: InodeId,
    pub parent_id: InodeId,
    pub name: NameBuf,
    pub active: bool,
}

impl TxOp {
    pub const fn empty() -> Self {
        Self {
            kind: TxOpKind::Create,
            vnode_id: INVALID_ID,
            parent_id: INVALID_ID,
            name: NameBuf::empty(),
            active: false,
        }
    }
}

#[derive(Clone, Copy)]
pub struct Transaction {
    pub id: u8,
    pub state: TxState,
    pub ops: [TxOp; MAX_TX_OPS],
    pub op_count: u8,
    pub pid: u16,
}

impl Transaction {
    pub const fn empty() -> Self {
        Self {
            id: INVALID_U8,
            state: TxState::Idle,
            ops: [TxOp::empty(); MAX_TX_OPS],
            op_count: 0,
            pid: 0,
        }
    }

    pub fn add_op(
        &mut self,
        kind: TxOpKind,
        vnode_id: InodeId,
        parent_id: InodeId,
        name: &str,
    ) -> VfsResult<()> {
        if self.state != TxState::Active {
            return Err(VfsError::InvalidArgument);
        }
        if self.op_count as usize >= MAX_TX_OPS {
            return Err(VfsError::NoSpace);
        }
        let idx = self.op_count as usize;
        self.ops[idx] = TxOp {
            kind,
            vnode_id,
            parent_id,
            name: NameBuf::from_str(name),
            active: true,
        };
        self.op_count += 1;
        Ok(())
    }
}

pub struct TxManager {
    pub transactions: [Transaction; MAX_TRANSACTIONS],
    pub next_id: u8,
}

impl TxManager {
    pub const fn new() -> Self {
        Self {
            transactions: [Transaction::empty(); MAX_TRANSACTIONS],
            next_id: 0,
        }
    }

    pub fn begin(&mut self, pid: u16) -> VfsResult<u8> {
        for (i, tx) in self.transactions.iter_mut().enumerate() {
            if tx.state == TxState::Idle
                || tx.state == TxState::Committed
                || tx.state == TxState::Aborted
            {
                tx.id = i as u8;
                tx.state = TxState::Active;
                tx.op_count = 0;
                tx.pid = pid;
                for op in tx.ops.iter_mut() {
                    *op = TxOp::empty();
                }
                self.next_id += 1;
                return Ok(i as u8);
            }
        }
        Err(VfsError::NoSpace)
    }

    pub fn commit(&mut self, tx_id: u8) -> VfsResult<()> {
        let i = tx_id as usize;
        if i >= MAX_TRANSACTIONS {
            return Err(VfsError::InvalidArgument);
        }
        if self.transactions[i].state != TxState::Active {
            return Err(VfsError::InvalidArgument);
        }
        self.transactions[i].state = TxState::Committed;
        Ok(())
    }

    pub fn abort(&mut self, tx_id: u8) -> VfsResult<()> {
        let i = tx_id as usize;
        if i >= MAX_TRANSACTIONS {
            return Err(VfsError::InvalidArgument);
        }
        if self.transactions[i].state != TxState::Active {
            return Err(VfsError::InvalidArgument);
        }
        self.transactions[i].state = TxState::Aborted;
        Ok(())
    }

    pub fn get(&self, tx_id: u8) -> Option<&Transaction> {
        let i = tx_id as usize;
        if i < MAX_TRANSACTIONS && self.transactions[i].state == TxState::Active {
            Some(&self.transactions[i])
        } else {
            None
        }
    }

    pub fn active_count(&self) -> usize {
        self.transactions
            .iter()
            .filter(|t| t.state == TxState::Active)
            .count()
    }
}
