// mikuD targets - runlevels for service activation

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Target {
    SysInit = 0,
    MultiUser = 1,
    Graphical = 2,
    Rescue = 3,
}

impl Target {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SysInit => "sysinit",
            Self::MultiUser => "multi-user",
            Self::Graphical => "graphical",
            Self::Rescue => "rescue",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "sysinit" | "sys-init" => Some(Self::SysInit),
            "multi-user" | "multiuser" | "multi" => Some(Self::MultiUser),
            "graphical" | "gui" => Some(Self::Graphical),
            "rescue" | "single" => Some(Self::Rescue),
            _ => None,
        }
    }
}
