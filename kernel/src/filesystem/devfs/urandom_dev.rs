use crate::driver::base::device::device_number::DeviceNumber;
use crate::filesystem::vfs::file::FileMode;
use crate::filesystem::vfs::syscall::ModeType;
use crate::filesystem::vfs::{
    vcore::generate_inode_id, FilePrivateData, FileSystem, FileType, IndexNode, Metadata,
};
use crate::libs::spinlock::SpinLockGuard;
use crate::{libs::spinlock::SpinLock, time::PosixTimeSpec};
use alloc::{
    string::String,
    sync::{Arc, Weak},
    vec::Vec,
};
use system_error::SystemError;
// use uuid::{uuid, Uuid};
use super::{DevFS, DeviceINode};

#[derive(Debug)]
pub struct UrandomInode {
    /// 指向自身的弱引用
    self_ref: Weak<LockedUrandomInode>,
    /// 指向inode所在的文件系统对象的指针
    fs: Weak<DevFS>,
    /// INode 元数据
    metadata: Metadata,
}

#[derive(Debug)]
pub struct LockedUrandomInode(SpinLock<UrandomInode>);

impl LockedUrandomInode {
    pub fn new() -> Arc<Self> {
        let inode = UrandomInode {
            self_ref: Weak::default(),
            fs: Weak::default(),
            metadata: Metadata {
                dev_id: 1,
                inode_id: generate_inode_id(),
                size: 0,
                blk_size: 0,
                blocks: 0,
                atime: PosixTimeSpec::default(),
                mtime: PosixTimeSpec::default(),
                ctime: PosixTimeSpec::default(),
                btime: PosixTimeSpec::default(),
                file_type: FileType::CharDevice,
                mode: ModeType::from_bits_truncate(0o666),
                nlinks: 1,
                uid: 0,
                gid: 0,
                raw_dev: DeviceNumber::default(), // 这里用来作为device number
            },
        };

        let result = Arc::new(LockedUrandomInode(SpinLock::new(inode)));
        result.0.lock().self_ref = Arc::downgrade(&result);

        return result;
    }
}

impl DeviceINode for LockedUrandomInode {
    fn set_fs(&self, fs: Weak<DevFS>) {
        self.0.lock().fs = fs;
    }
}

impl IndexNode for LockedUrandomInode {
    fn as_any_ref(&self) -> &dyn core::any::Any {
        self
    }

    fn open(
        &self,
        _data: SpinLockGuard<FilePrivateData>,
        _mode: &FileMode,
    ) -> Result<(), SystemError> {
        return Ok(());
    }

    fn close(&self, _data: SpinLockGuard<FilePrivateData>) -> Result<(), SystemError> {
        return Ok(());
    }

    fn metadata(&self) -> Result<Metadata, SystemError> {
        return Ok(self.0.lock().metadata.clone());
    }

    fn fs(&self) -> Arc<dyn FileSystem> {
        return self.0.lock().fs.upgrade().unwrap();
    }

    fn list(&self) -> Result<Vec<String>, SystemError> {
        Err(SystemError::ENOSYS)
    }

    fn set_metadata(&self, metadata: &Metadata) -> Result<(), SystemError> {
        let mut inode = self.0.lock();
        inode.metadata.atime = metadata.atime;
        inode.metadata.mtime = metadata.mtime;
        inode.metadata.ctime = metadata.ctime;
        inode.metadata.btime = metadata.btime;
        inode.metadata.mode = metadata.mode;
        inode.metadata.uid = metadata.uid;
        inode.metadata.gid = metadata.gid;

        return Ok(());
    }

    fn read_at(
        &self,
        offset: usize,
        len: usize,
        buf: &mut [u8],
        _data: SpinLockGuard<FilePrivateData>,
    ) -> Result<usize, SystemError> {
        if offset != 0 {
            // urandom 通常不支持随机访问，从非0位置读取可能返回错误
            return Err(SystemError::ESPIPE);
        }

        if buf.len() < len {
            return Err(SystemError::EINVAL);
        }

        // 生成随机数据填充缓冲区
        // 这里需要实现随机数生成器，可以使用硬件RNG或软件PRNG
        self.fill_random(&mut buf[..len]);

        Ok(len)
    }

    fn write_at(
        &self,
        _offset: usize,
        len: usize,
        buf: &[u8],
        _data: SpinLockGuard<FilePrivateData>,
    ) -> Result<usize, SystemError> {
        // urandom 设备通常可写（用于向熵池添加熵），但这里简单实现
        if buf.len() < len {
            return Err(SystemError::EINVAL);
        }

        Ok(len)
    }
}

impl LockedUrandomInode {
    /// 填充随机数据到缓冲区
    fn fill_random(&self, buf: &mut [u8]) {
        // 这里需要实现随机数生成
        // 在no_std环境中，可以使用：
        // 1. 硬件随机数生成器（如果可用）
        // 2. 软件伪随机数生成器

        // 简单示例：使用零填充（实际使用时需要替换为真正的随机数生成）
        for byte in buf.iter_mut() {
            *byte = 0; // 这里应该替换为随机字节
        }
    }
}
