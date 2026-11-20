#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    Ok,
    Failure,
    Usage,
    Auth,
    Quota,
}

impl Exit {
    pub fn code(self) -> i32 {
        match self {
            Exit::Ok => 0,
            Exit::Failure => 1,
            Exit::Usage => 2,
            Exit::Auth => 3,
            Exit::Quota => 4,
        }
    }

    pub fn from_code(code: i64) -> Exit {
        match code {
            0 => Exit::Ok,
            2 => Exit::Usage,
            3 => Exit::Auth,
            4 => Exit::Quota,
            _ => Exit::Failure,
        }
    }
}
