use alloc::string::ToString;
use alloc::vec::Vec;
use core::ffi::c_int;
use core::usize;

use crate::arch::interrupt::TrapFrame;
use crate::syscall::table::FormattedSyscallParam;
use crate::syscall::table::Syscall;
use crate::{
    arch::{ipc::signal::Signal, syscall::nr::SYS_TGKILL},
    process::{process_group::Pgid, Pid, ProcessManager},
};
use log::warn;
use system_error::SystemError;

use crate::ipc::kill::{kill_all, kill_process, kill_process_group};

/// ### pid转换器，将输入的id转换成对应的pid或pgid
/// - 如果id < -1，则为pgid
/// - 如果id == -1，则为所有进程
/// - 如果id == 0，则为当前进程组
/// - 如果id > 0，则为pid
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PidConverter {
    All,
    Pid(Pid),
    Pgid(Pgid),
}

impl PidConverter {
    /// ### 为 `wait` 和 `kill` 调用使用
    pub fn from_id(id: i32) -> Self {
        if id < -1 {
            PidConverter::Pgid(Pgid::from(-id as usize))
        } else if id == -1 {
            PidConverter::All
        } else if id == 0 {
            let pgid = ProcessManager::current_pcb().pgid();
            PidConverter::Pgid(pgid)
        } else {
            PidConverter::Pid(Pid::from(id as usize))
        }
    }
}

pub struct SysTgKillHandle;

impl SysTgKillHandle {
    #[inline(always)]
    fn tgid(args: &[usize]) -> i32 {
        // 第一个参数是tgid(pid)
        args[0] as i32
    }
    #[inline(always)]
    fn tid(args: &[usize]) -> i32 {
        // 第二个参数是tid
        args[1] as i32
    }
    #[inline(always)]
    fn sig(args: &[usize]) -> c_int {
        // 第三个参数是信号值
        args[2] as c_int
    }
}

impl Syscall for SysTgKillHandle {
    fn num_args(&self) -> usize {
        3
    }

    fn handle(&self, args: &[usize], _frame: &mut TrapFrame) -> Result<usize, SystemError> {
        let tgid = Pgid::from(Self::tgid(args) as usize);
        let tid = Pid::from(Self::tid(args) as usize);
        let sig_c_int = Self::sig(args);

        let pg = ProcessManager::find_process_group(tgid).ok_or(SystemError::ESRCH)?;
        if pg.contains(tid) {
            let converter = PidConverter::from_id(tid.data() as i32);
            let sig = Signal::from(sig_c_int);
            if sig == Signal::INVALID {
                warn!("Not a valid signal number");
                return Err(SystemError::EINVAL);
            }

            match converter {
                PidConverter::Pid(pid) => kill_process(pid, sig),
                PidConverter::Pgid(pgid) => kill_process_group(pgid, sig),
                PidConverter::All => kill_all(sig),
            }
        } else {
            Err(SystemError::EINVAL)
        }
    }

    fn entry_format(&self, args: &[usize]) -> Vec<FormattedSyscallParam> {
        vec![
            FormattedSyscallParam::new("tgid", Self::tgid(args).to_string()),
            FormattedSyscallParam::new("tid", Self::tid(args).to_string()),
            FormattedSyscallParam::new("sig", Self::sig(args).to_string()),
        ]
    }
}

// 注册系统调用
syscall_table_macros::declare_syscall!(SYS_TGKILL, SysTgKillHandle);
