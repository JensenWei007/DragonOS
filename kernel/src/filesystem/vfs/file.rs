use core::sync::atomic::{AtomicUsize, Ordering};

use alloc::{string::String, sync::Arc, vec::Vec};
use log::error;
use system_error::SystemError;

use super::file_operations::FileOperations;
use super::{FileType, IndexNode, InodeId, Metadata, SpecialNodeData};
use crate::process::pid::PidPrivateData;
use crate::{
    driver::{
        base::{block::SeekFrom, device::DevicePrivateData},
        tty::tty_device::TtyFilePrivateData,
    },
    filesystem::{
        epoll::{event_poll::EPollPrivateData, EPollItem},
        procfs::ProcfsFilePrivateData,
        vfs::FilldirContext,
    },
    ipc::pipe::PipeFsPrivateData,
    libs::{rwlock::RwLock, spinlock::SpinLock},
    process::{cred::Cred, ProcessManager},
};

/// 文件私有信息的枚举类型
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum FilePrivateData {
    /// 管道文件私有信息
    Pipefs(PipeFsPrivateData),
    /// procfs文件私有信息
    Procfs(ProcfsFilePrivateData),
    /// 设备文件的私有信息
    DevFS(DevicePrivateData),
    /// tty设备文件的私有信息
    Tty(TtyFilePrivateData),
    /// epoll私有信息
    EPoll(EPollPrivateData),
    /// pid私有信息
    Pid(PidPrivateData),
    /// 不需要文件私有信息
    Unused,
}

impl Default for FilePrivateData {
    fn default() -> Self {
        return Self::Unused;
    }
}

impl FilePrivateData {
    pub fn update_mode(&mut self, mode: FileMode) {
        if let FilePrivateData::Pipefs(pdata) = self {
            pdata.set_mode(mode);
        }
    }

    pub fn is_pid(&self) -> bool {
        if let FilePrivateData::Pid(_data) = self {
            return true;
        }
        false
    }

    pub fn get_pid(&self) -> i32 {
        if let FilePrivateData::Pid(data) = self {
            return data.pid();
        }
        -1
    }
}

bitflags! {
    /// @brief 文件打开模式
    /// 其中，低2bit组合而成的数字的值，用于表示访问权限。其他的bit，才支持通过按位或的方式来表示参数
    ///
    /// 与Linux 5.19.10的uapi/asm-generic/fcntl.h相同
    /// https://code.dragonos.org.cn/xref/linux-5.19.10/tools/include/uapi/asm-generic/fcntl.h#19
    #[allow(clippy::bad_bit_mask)]
    pub struct FileMode: u32{
        /* File access modes for `open' and `fcntl'.  */
        /// Open Read-only
        const O_RDONLY = 0o0;
        /// Open Write-only
        const O_WRONLY = 0o1;
        /// Open read/write
        const O_RDWR = 0o2;
        /// Mask for file access modes
        const O_ACCMODE = 0o00000003;

        /* Bits OR'd into the second argument to open.  */
        /// Create file if it does not exist
        const O_CREAT = 0o00000100;
        /// Fail if file already exists
        const O_EXCL = 0o00000200;
        /// Do not assign controlling terminal
        const O_NOCTTY = 0o00000400;
        /// 文件存在且是普通文件，并以O_RDWR或O_WRONLY打开，则它会被清空
        const O_TRUNC = 0o00001000;
        /// 文件指针会被移动到文件末尾
        const O_APPEND = 0o00002000;
        /// 非阻塞式IO模式
        const O_NONBLOCK = 0o00004000;
        /// 每次write都等待物理I/O完成，但是如果写操作不影响读取刚写入的数据，则不等待文件属性更新
        const O_DSYNC = 0o00010000;
        /// fcntl, for BSD compatibility
        const FASYNC = 0o00020000;
        /* direct disk access hint */
        const O_DIRECT = 0o00040000;
        const O_LARGEFILE = 0o00100000;
        /// 打开的必须是一个目录
        const O_DIRECTORY = 0o00200000;
        /// Do not follow symbolic links
        const O_NOFOLLOW = 0o00400000;
        const O_NOATIME = 0o01000000;
        /// set close_on_exec
        const O_CLOEXEC = 0o02000000;
        /// 每次write都等到物理I/O完成，包括write引起的文件属性的更新
        const O_SYNC = 0o04000000;

        const O_PATH = 0o10000000;

        const O_PATH_FLAGS = Self::O_DIRECTORY.bits|Self::O_NOFOLLOW.bits|Self::O_CLOEXEC.bits|Self::O_PATH.bits;
    }
}

impl FileMode {
    /// @brief 获取文件的访问模式的值
    #[inline]
    pub fn accmode(&self) -> u32 {
        return self.bits() & FileMode::O_ACCMODE.bits();
    }
}

/// @brief 抽象文件结构体
#[derive(Debug)]
pub struct File {
    inode: Arc<dyn IndexNode>,
    /// 对于文件，表示字节偏移量；对于文件夹，表示当前操作的子目录项偏移量
    offset: AtomicUsize,
    /// 文件的打开模式
    mode: RwLock<FileMode>,
    /// 文件类型
    file_type: FileType,
    /// readdir时候用的，暂存的本次循环中，所有子目录项的名字的数组
    readdir_subdirs_name: SpinLock<Vec<String>>,
    pub private_data: SpinLock<FilePrivateData>,
    /// 文件的凭证
    cred: Cred,
}

impl File {
    /// @brief 创建一个新的文件对象
    ///
    /// @param inode 文件对象对应的inode
    /// @param mode 文件的打开模式
    pub fn new(inode: Arc<dyn IndexNode>, mode: FileMode) -> Result<Self, SystemError> {
        let mut inode = inode;
        let file_type = inode.metadata()?.file_type;
        if file_type == FileType::Pipe {
            if let Some(SpecialNodeData::Pipe(pipe_inode)) = inode.special_node() {
                inode = pipe_inode;
            }
        }

        let f = File {
            inode,
            offset: AtomicUsize::new(0),
            mode: RwLock::new(mode),
            file_type,
            readdir_subdirs_name: SpinLock::new(Vec::default()),
            private_data: SpinLock::new(FilePrivateData::default()),
            cred: ProcessManager::current_pcb().cred(),
        };
        f.inode.open(f.private_data.lock(), &mode)?;

        return Ok(f);
    }

    fn do_read(
        &self,
        offset: usize,
        len: usize,
        buf: &mut [u8],
        update_offset: bool,
    ) -> Result<usize, SystemError> {
        // 先检查本文件在权限等规则下，是否可读取。
        self.readable()?;
        if buf.len() < len {
            return Err(SystemError::ENOBUFS);
        }

        let len = if self.mode().contains(FileMode::O_DIRECT) {
            self.inode
                .read_direct(offset, len, buf, self.private_data.lock())
        } else {
            self.inode
                .read_at(offset, len, buf, self.private_data.lock())
        }?;

        if update_offset {
            self.offset
                .fetch_add(len, core::sync::atomic::Ordering::SeqCst);
        }

        Ok(len)
    }

    fn do_write(
        &self,
        offset: usize,
        len: usize,
        buf: &[u8],
        update_offset: bool,
    ) -> Result<usize, SystemError> {
        // 先检查本文件在权限等规则下，是否可写入。
        self.writeable()?;
        if buf.len() < len {
            return Err(SystemError::ENOBUFS);
        }

        // 如果文件指针已经超过了文件大小，则需要扩展文件大小
        if offset > self.inode.metadata()?.size as usize {
            self.inode.resize(offset)?;
        }
        let len = self
            .inode
            .write_at(offset, len, buf, self.private_data.lock())?;

        if update_offset {
            self.offset
                .fetch_add(len, core::sync::atomic::Ordering::SeqCst);
        }

        Ok(len)
    }
}

impl FileOperations for File {
    fn read(&self, len: usize, buf: &mut [u8]) -> Result<usize, SystemError> {
        self.do_read(self.offset.load(Ordering::SeqCst), len, buf, true)
    }

    fn write(&self, len: usize, buf: &[u8]) -> Result<usize, SystemError> {
        self.do_write(self.offset.load(Ordering::SeqCst), len, buf, true)
    }

    fn pread(&self, offset: usize, len: usize, buf: &mut [u8]) -> Result<usize, SystemError> {
        self.do_read(offset, len, buf, false)
    }

    fn pwrite(&self, offset: usize, len: usize, buf: &[u8]) -> Result<usize, SystemError> {
        self.do_write(offset, len, buf, false)
    }

    fn lseek(&self, origin: SeekFrom) -> Result<usize, SystemError> {
        let file_type = self.inode.metadata()?.file_type;
        match file_type {
            FileType::Pipe | FileType::CharDevice => {
                return Err(SystemError::ESPIPE);
            }
            _ => {}
        }

        let pos: i64 = match origin {
            SeekFrom::SeekSet(offset) => offset,
            SeekFrom::SeekCurrent(offset) => self.offset.load(Ordering::SeqCst) as i64 + offset,
            SeekFrom::SeekEnd(offset) => {
                let metadata = self.metadata()?;
                metadata.size + offset
            }
            SeekFrom::Invalid => {
                return Err(SystemError::EINVAL);
            }
        };
        // 根据linux man page, lseek允许超出文件末尾，并且不改变文件大小
        // 当pos超出文件末尾时，read返回0。直到开始写入数据时，才会改变文件大小
        if pos < 0 {
            return Err(SystemError::EOVERFLOW);
        }
        self.offset.store(pos as usize, Ordering::SeqCst);
        return Ok(pos as usize);
    }

    fn metadata(&self) -> Result<Metadata, SystemError> {
        self.inode.metadata()
    }

    fn get_entry_name(&self, ino: InodeId) -> Result<String, SystemError> {
        self.inode.get_entry_name(ino)
    }

    fn read_dir(&self, ctx: &mut FilldirContext) -> Result<(), SystemError> {
        let inode: &Arc<dyn IndexNode> = &self.inode;
        let mut current_pos = self.offset.load(Ordering::SeqCst);

        // POSIX 标准要求readdir应该返回. 和 ..
        // 但是观察到在现有的子目录中已经包含，不做处理也能正常返回. 和 .. 这里先不做处理

        // 迭代读取目录项
        let readdir_subdirs_name = inode.list()?;

        let subdirs_name_len = readdir_subdirs_name.len();
        while current_pos < subdirs_name_len {
            let name = &readdir_subdirs_name[current_pos];
            let sub_inode: Arc<dyn IndexNode> = match inode.find(name) {
                Ok(i) => i,
                Err(e) => {
                    error!("Readdir error: Failed to find sub inode");
                    return Err(e);
                }
            };

            let inode_metadata = sub_inode.metadata().unwrap();
            let entry_ino = inode_metadata.inode_id.into() as u64;
            let entry_d_type = inode_metadata.file_type.get_file_type_num() as u8;
            match ctx.fill_dir(name, current_pos, entry_ino, entry_d_type) {
                Ok(_) => {
                    self.offset.fetch_add(1, Ordering::SeqCst);
                    current_pos += 1;
                }
                Err(SystemError::EINVAL) => {
                    return Ok(());
                }
                Err(e) => {
                    ctx.error = Some(e.clone());
                    return Err(e);
                }
            }
        }
        return Ok(());
    }

    fn readable(&self) -> Result<(), SystemError> {
        if *self.mode.read() == FileMode::O_WRONLY {
            return Err(SystemError::EPERM);
        }
        Ok(())
    }

    fn writeable(&self) -> Result<(), SystemError> {
        if *self.mode.read() == FileMode::O_RDONLY {
            return Err(SystemError::EPERM);
        }
        Ok(())
    }

    fn close_on_exec(&self) -> bool {
        self.mode().contains(FileMode::O_CLOEXEC)
    }

    fn set_close_on_exec(&self, close_on_exec: bool) {
        let mut mode_guard = self.mode.write();
        if close_on_exec {
            mode_guard.insert(FileMode::O_CLOEXEC);
        } else {
            mode_guard.remove(FileMode::O_CLOEXEC);
        }
    }

    fn ftruncate(&self, len: usize) -> Result<(), SystemError> {
        self.writeable()?;
        self.inode.resize(len)
    }

    fn add_epitem(&self, epitem: Arc<EPollItem>) -> Result<(), SystemError> {
        let private_data = self.private_data.lock();
        self.inode
            .as_pollable_inode()?
            .add_epitem(epitem, &private_data)
    }

    fn remove_epitem(&self, epitem: &Arc<EPollItem>) -> Result<(), SystemError> {
        let private_data = self.private_data.lock();
        self.inode
            .as_pollable_inode()?
            .remove_epitem(epitem, &private_data)
    }

    fn poll(&self) -> Result<usize, SystemError> {
        let private_data = self.private_data.lock();
        self.inode.as_pollable_inode()?.poll(&private_data)
    }

    fn file_type(&self) -> FileType {
        self.file_type
    }

    fn mode(&self) -> FileMode {
        *self.mode.read()
    }

    fn set_mode(&self, mode: FileMode) -> Result<(), SystemError> {
        *self.mode.write() = mode;
        self.private_data.lock().update_mode(mode);
        Ok(())
    }

    fn try_clone(&self) -> Option<Arc<dyn FileOperations>> {
        let cloned_file = File {
            inode: self.inode.clone(),
            offset: AtomicUsize::new(self.offset.load(Ordering::SeqCst)),
            mode: RwLock::new(self.mode()),
            file_type: self.file_type,
            readdir_subdirs_name: SpinLock::new(self.readdir_subdirs_name.lock().clone()),
            private_data: SpinLock::new(self.private_data.lock().clone()),
            cred: self.cred.clone(),
        };

        if self
            .inode
            .open(cloned_file.private_data.lock(), &cloned_file.mode())
            .is_err()
        {
            return None;
        }

        Some(Arc::new(cloned_file))
    }

    fn inode(&self) -> Arc<dyn IndexNode> {
        self.inode.clone()
    }

    fn offset(&self) -> usize {
        self.offset.load(Ordering::SeqCst)
    }

    fn set_offset(&self, offset: usize) {
        self.offset.store(offset, Ordering::SeqCst);
    }
}

impl Drop for File {
    fn drop(&mut self) {
        let r: Result<(), SystemError> = self.inode.close(self.private_data.lock());
        // 打印错误信息
        if r.is_err() {
            error!(
                "pid: {:?} failed to close file: {:?}, errno={:?}",
                ProcessManager::current_pcb().pid(),
                self,
                r.as_ref().unwrap_err()
            );
        }
    }
}

/// @brief pcb里面的文件描述符数组
#[derive(Debug)]
pub struct FileDescriptorVec {
    /// 当前进程打开的文件描述符（使用trait object）
    fds: Vec<Option<Arc<dyn FileOperations>>>,
}

impl Default for FileDescriptorVec {
    fn default() -> Self {
        Self::new()
    }
}
impl FileDescriptorVec {
    pub const PROCESS_MAX_FD: usize = 1024;

    #[inline(never)]
    pub fn new() -> FileDescriptorVec {
        let mut data = Vec::with_capacity(FileDescriptorVec::PROCESS_MAX_FD);
        data.resize(FileDescriptorVec::PROCESS_MAX_FD, None);

        // 初始化文件描述符数组结构体
        return FileDescriptorVec { fds: data };
    }

    /// @brief 克隆一个文件描述符数组
    ///
    /// @return FileDescriptorVec 克隆后的文件描述符数组
    pub fn clone(&self) -> FileDescriptorVec {
        let mut res = FileDescriptorVec::new();
        for i in 0..FileDescriptorVec::PROCESS_MAX_FD {
            if let Some(file) = &self.fds[i] {
                if let Some(file) = file.try_clone() {
                    res.fds[i] = Some(file);
                }
            }
        }
        return res;
    }

    /// 返回 `已经打开的` 文件描述符的数量
    pub fn fd_open_count(&self) -> usize {
        let mut size = 0;
        for fd in &self.fds {
            if fd.is_some() {
                size += 1;
            }
        }
        return size;
    }

    /// @brief 判断文件描述符序号是否合法
    ///
    /// @return true 合法
    ///
    /// @return false 不合法
    #[inline]
    pub fn validate_fd(fd: i32) -> bool {
        return !(fd < 0 || fd as usize > FileDescriptorVec::PROCESS_MAX_FD);
    }

    /// 申请文件描述符，并把文件对象存入其中。
    ///
    /// ## 参数
    ///
    /// - `file` 要存放的文件对象
    /// - `fd` 如果为Some(i32)，表示指定要申请这个文件描述符，如果这个文件描述符已经被使用，那么返回EBADF
    ///
    /// ## 返回值
    ///
    /// - `Ok(i32)` 申请成功，返回申请到的文件描述符
    /// - `Err(SystemError)` 申请失败，返回错误码，并且，file对象将被drop掉
    pub fn alloc_fd(
        &mut self,
        file: Arc<dyn FileOperations>,
        fd: Option<i32>,
    ) -> Result<i32, SystemError> {
        if let Some(new_fd) = fd {
            let x = &mut self.fds[new_fd as usize];
            if x.is_none() {
                *x = Some(file);
                return Ok(new_fd);
            } else {
                return Err(SystemError::EBADF);
            }
        } else {
            for i in 0..FileDescriptorVec::PROCESS_MAX_FD {
                if self.fds[i].is_none() {
                    self.fds[i] = Some(file);
                    return Ok(i as i32);
                }
            }
            return Err(SystemError::EMFILE);
        }
    }

    /// 根据文件描述符序号，获取文件结构体的Arc指针
    ///
    /// ## 参数
    ///
    /// - `fd` 文件描述符序号
    pub fn get_file_by_fd(&self, fd: i32) -> Option<Arc<dyn FileOperations>> {
        if !FileDescriptorVec::validate_fd(fd) {
            return None;
        }
        self.fds[fd as usize].clone()
    }

    /// 释放文件描述符，同时关闭文件。
    ///
    /// ## 参数
    ///
    /// - `fd` 文件描述符序号
    pub fn drop_fd(&mut self, fd: i32) -> Result<Arc<dyn FileOperations>, SystemError> {
        self.get_file_by_fd(fd).ok_or(SystemError::EBADF)?;

        // 把文件描述符数组对应位置设置为空
        let file = self.fds[fd as usize].take().unwrap();
        return Ok(file);
    }

    #[allow(dead_code)]
    pub fn iter(&self) -> FileDescriptorIterator {
        return FileDescriptorIterator::new(self);
    }

    pub fn close_on_exec(&mut self) {
        for i in 0..FileDescriptorVec::PROCESS_MAX_FD {
            if let Some(file) = &self.fds[i] {
                let to_drop = file.close_on_exec();
                if to_drop {
                    if let Err(r) = self.drop_fd(i as i32) {
                        error!(
                            "Failed to close file: pid = {:?}, fd = {}, error = {:?}",
                            ProcessManager::current_pcb().pid(),
                            i,
                            r
                        );
                    }
                }
            }
        }
    }
}

#[derive(Debug)]
pub struct FileDescriptorIterator<'a> {
    fds: &'a FileDescriptorVec,
    index: usize,
}

impl<'a> FileDescriptorIterator<'a> {
    pub fn new(fds: &'a FileDescriptorVec) -> Self {
        return Self { fds, index: 0 };
    }
}

impl Iterator for FileDescriptorIterator<'_> {
    type Item = (i32, Arc<File>);

    fn next(&mut self) -> Option<Self::Item> {
        while self.index < FileDescriptorVec::PROCESS_MAX_FD {
            let fd = self.index as i32;
            self.index += 1;
            if let Some(file) = self.fds.get_file_by_fd(fd) {
                let file = file.downcast_arc::<File>().unwrap();
                return Some((fd, file));
            }
        }
        return None;
    }
}
