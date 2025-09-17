#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum PidType {
    /// pid类型是进程id
    PID = 1,
    TGID = 2,
    PGID = 3,
    SID = 4,
    MAX = 5,
}

/// 为PidType实现判断相等的trait
impl PartialEq for PidType {
    fn eq(&self, other: &PidType) -> bool {
        *self as u8 == *other as u8
    }
}

/// 每个进程的 pid 私有信息, 通常作为 pidfd 的 private_data
/// TODO: 未实现完, 参考https://code.dragonos.org.cn/xref/linux-6.1.9/include/linux/pid.h#59
/// TODO: 应该替换所有的 pid 相关使用, 目前内核是直接使用传入的 pid, 应该全部转换为使用此结构体
/// 例如 struct pid 应该是在进程创建时（如 fork(), clone()）必然创建的
#[derive(Clone, Debug)]
pub struct PidPrivateData {
    pid: i32,
    ref_count: usize,
}

impl PidPrivateData {
    pub fn new(pid: i32) -> Self {
        Self {
            pid: pid,
            ref_count: 0,
        }
    }

    pub fn pid(&self) -> i32 {
        self.pid
    }
}
