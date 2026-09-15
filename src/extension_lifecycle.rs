//! Explicit local installation of the embedded Pi extension. This is outside
//! collection: lifecycle commands never start a collector or read ptop config.

#[cfg(unix)]
pub const ASSET: &[u8] = include_bytes!("../assets/pi-extension/ptop-live-harness.ts");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Command {
    Install,
    Remove,
    Status,
}

#[cfg(not(unix))]
pub fn execute(_: Command) -> Result<String, String> {
    Err("ptop extension lifecycle is unsupported on Windows; no files were changed".to_string())
}

#[cfg(all(test, not(unix)))]
mod unsupported_tests {
    use super::*;

    #[test]
    fn parsed_lifecycle_commands_are_unsupported_without_mutation() {
        for command in [Command::Install, Command::Status, Command::Remove] {
            assert_eq!(
                execute(command),
                Err(
                    "ptop extension lifecycle is unsupported on Windows; no files were changed"
                        .to_string()
                )
            );
        }
    }
}

#[cfg(unix)]
mod unix {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::ffi::{OsStr, OsString};
    use std::fs::File;
    use std::io::{Read, Write};
    use std::os::fd::{AsRawFd, FromRawFd, RawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::path::{Component, Path, PathBuf};
    use std::time::{Duration, Instant};

    const TARGET: &[u8] = b"ptop-live-harness.ts\0";
    const INSTALL_TEMP: &[u8] = b".ptop-live-install\0";
    const LOCK: &[u8] = b".ptop-live-harness.lock\0";
    const MAX_BYTES: usize = 64 * 1024;
    const MAX_PRIOR_OFFICIAL: usize = 8;
    // Current asset plus capacity for exactly eight prior official releases.
    // No historic digest is fabricated for this first lifecycle release.
    const PRIOR_OFFICIAL: [(&str, [u8; 32]); 0] = [];

    #[cfg(test)]
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum TestFault {
        PostLinkTargetIdentity,
        PostLinkTempIdentity,
        PostLinkPairMismatch,
        PostLinkResidueUnlink,
        DirectorySyncEinval,
        DirectorySyncError,
        BeforeLinkTargetAppear,
        BeforeRemoveReplaceTarget,
        BeforeRemoveModifyBytes,
        BeforeRemoveModifyMode,
        BeforeRevalidateGrow,
        BeforeRevalidateTruncate,
        BeforeRevalidateReplaceContent,
    }
    #[cfg(test)]
    std::thread_local! {
        static TEST_FAULT: std::cell::Cell<Option<TestFault>> = const { std::cell::Cell::new(None) };
        // Lets tests exercise root-owned path handling without chown privileges.
        static TEST_ALL_DIRECTORY_OWNERS_ROOT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    #[cfg(test)]
    fn take_test_fault(fault: TestFault) -> bool {
        TEST_FAULT.with(|configured| {
            if configured.get() == Some(fault) {
                configured.set(None);
                true
            } else {
                false
            }
        })
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct Identity {
        dev: libc::dev_t,
        ino: libc::ino_t,
        mode: libc::mode_t,
        uid: libc::uid_t,
        nlink: libc::nlink_t,
        size: libc::off_t,
    }
    struct Dir {
        file: File,
        agent: PathBuf,
        chain: Vec<Identity>,
    }
    impl Dir {
        fn fd(&self) -> RawFd {
            self.file.as_raw_fd()
        }
    }
    /// A bounded opened file whose pathname is rechecked before mutation.
    struct VerifiedFile {
        file: File,
        identity: Identity,
        bytes: Vec<u8>,
        digest: [u8; 32],
    }

    pub(super) fn execute(command: Command) -> Result<String, String> {
        let home = dirs::home_dir().ok_or("cannot resolve home directory for Pi extension")?;
        let agent = effective_agent_dir(&home, std::env::var_os("PI_CODING_AGENT_DIR"))?;
        match command {
            Command::Status => status(&agent),
            Command::Install | Command::Remove => {
                let directory = walk_agent_dir(&agent, true)?;
                let lock = Lock::acquire(&directory)?;
                directory.revalidate()?;
                lock.revalidate(&directory)?;
                match command {
                    Command::Install => install(&directory, &lock),
                    Command::Remove => remove(&directory, &lock),
                    Command::Status => unreachable!(),
                }
            }
        }
    }

    fn effective_agent_dir(home: &Path, configured: Option<OsString>) -> Result<PathBuf, String> {
        if !home.is_absolute() || home.as_os_str().as_bytes().contains(&0) {
            return Err(
                "cannot resolve a valid absolute home directory for Pi extension".to_string(),
            );
        }
        let path = match configured.filter(|v| !v.is_empty()) {
            None => home.join(".pi/agent"),
            Some(value) => {
                let text = value
                    .to_str()
                    .ok_or("PI_CODING_AGENT_DIR must be valid UTF-8")?;
                if text == "~" {
                    home.to_path_buf()
                } else if let Some(rest) = text.strip_prefix("~/") {
                    if rest.starts_with('/') {
                        return Err("PI_CODING_AGENT_DIR accepts only exact '~' or '~/relative' tilde forms".to_string());
                    }
                    home.join(rest)
                } else if text.contains('~') {
                    return Err(
                        "PI_CODING_AGENT_DIR accepts only exact '~' or '~/relative' tilde forms"
                            .to_string(),
                    );
                } else {
                    PathBuf::from(value)
                }
            }
        };
        if !path.is_absolute() || path.as_os_str().as_bytes().contains(&0) {
            return Err("Pi agent directory must be an absolute path".to_string());
        }
        let mut normalized = PathBuf::from("/");
        for component in path.components() {
            match component {
                Component::RootDir | Component::CurDir => {}
                Component::Normal(name) => normalized.push(name),
                Component::ParentDir => {
                    if normalized == Path::new("/") {
                        return Err("Pi agent directory escapes its absolute root".to_string());
                    }
                    normalized.pop();
                }
                _ => return Err("Pi agent directory contains an unsupported component".to_string()),
            }
        }
        Ok(normalized)
    }

    impl Dir {
        fn revalidate(&self) -> Result<(), String> {
            let now = walk_agent_dir(&self.agent, false)?;
            if now.chain.len() != self.chain.len()
                || now
                    .chain
                    .iter()
                    .zip(&self.chain)
                    .any(|(now, then)| !same_directory_identity(*now, *then))
                || !same_directory_identity(stat_fd(now.fd())?, stat_fd(self.fd())?)
            {
                Err("Pi directory path changed during lifecycle operation".to_string())
            } else {
                Ok(())
            }
        }
    }
    fn walk_agent_dir(agent: &Path, create: bool) -> Result<Dir, String> {
        let uid = unsafe { libc::geteuid() };
        let mut current = open_dir(libc::AT_FDCWD, b"/\0").map_err(|e| e.to_string())?;
        let mut chain = vec![stat_fd(current.fd())?];
        let mut user_anchor = false;
        for component in agent
            .components()
            .filter_map(|c| {
                if let Component::Normal(n) = c {
                    Some(n)
                } else {
                    None
                }
            })
            .chain([OsStr::new("extensions")])
        {
            let name = c_name(component)?;
            let next = match open_dir(current.fd(), &name) {
                Ok(dir) => dir,
                Err(error)
                    if error.raw_os_error() == Some(libc::ENOENT) && create && user_anchor =>
                {
                    mkdirat(current.fd(), &name, 0o700)?;
                    if sync_dir(&current).is_err() {
                        return Err("Pi directory was created but durability is uncertain; inspect it manually".to_string());
                    }
                    open_dir(current.fd(), &name).map_err(|e| e.to_string())?
                }
                Err(error) if error.raw_os_error() == Some(libc::ENOENT) => {
                    return Err("Pi extension directory is absent".to_string())
                }
                Err(error) => return Err(format!("cannot safely open Pi directory: {error}")),
            };
            validate_dir(&next, uid)?;
            let identity = stat_fd(next.fd())?;
            if directory_owner(identity.uid) == uid {
                user_anchor = true;
            }
            chain.push(identity);
            current = next;
        }
        Ok(Dir {
            file: current.file,
            agent: agent.to_path_buf(),
            chain,
        })
    }
    fn c_name(value: &OsStr) -> Result<Vec<u8>, String> {
        let mut b = value.as_bytes().to_vec();
        if b.is_empty() || b.contains(&0) || b.contains(&b'/') {
            return Err("invalid Pi directory component".to_string());
        }
        b.push(0);
        Ok(b)
    }
    fn open_dir(parent: RawFd, name: &[u8]) -> std::io::Result<Dir> {
        let fd = unsafe {
            libc::openat(
                parent,
                name.as_ptr().cast(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if fd < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(Dir {
                file: unsafe { File::from_raw_fd(fd) },
                agent: PathBuf::new(),
                chain: vec![],
            })
        }
    }
    fn mkdirat(parent: RawFd, name: &[u8], mode: u32) -> Result<(), String> {
        if unsafe { libc::mkdirat(parent, name.as_ptr().cast(), mode as libc::mode_t) } == 0 {
            Ok(())
        } else {
            Err(format!(
                "cannot create Pi directory: {}",
                std::io::Error::last_os_error()
            ))
        }
    }
    fn directory_owner(owner: libc::uid_t) -> libc::uid_t {
        #[cfg(test)]
        if TEST_ALL_DIRECTORY_OWNERS_ROOT.with(|enabled| enabled.get()) {
            return 0;
        }
        owner
    }
    fn validate_dir(dir: &Dir, uid: u32) -> Result<(), String> {
        let s = stat_fd(dir.fd())?;
        if s.mode & libc::S_IFMT != libc::S_IFDIR
            || s.mode & 0o022 != 0
            || (directory_owner(s.uid) != uid && directory_owner(s.uid) != 0)
        {
            Err("refusing unsafe Pi directory component".to_string())
        } else {
            Ok(())
        }
    }
    fn same_directory_identity(left: Identity, right: Identity) -> bool {
        left.dev == right.dev
            && left.ino == right.ino
            && left.mode == right.mode
            && left.uid == right.uid
    }
    fn stat_fd(fd: RawFd) -> Result<Identity, String> {
        let mut s = unsafe { std::mem::zeroed::<libc::stat>() };
        if unsafe { libc::fstat(fd, &mut s) } != 0 {
            return Err(format!(
                "cannot stat descriptor: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(Identity {
            dev: s.st_dev,
            ino: s.st_ino,
            mode: s.st_mode,
            uid: s.st_uid,
            nlink: s.st_nlink,
            size: s.st_size,
        })
    }
    fn digest(bytes: &[u8]) -> [u8; 32] {
        Sha256::digest(bytes).into()
    }
    #[cfg(test)]
    const TEST_PRIOR_ASSET: &[u8] = b"test-recognized-prior-asset";

    fn recognized(bytes: &[u8]) -> bool {
        bytes.len() <= MAX_BYTES
            && (digest(bytes) == digest(ASSET)
                || PRIOR_OFFICIAL.iter().any(|(_, d)| *d == digest(bytes))
                || {
                    #[cfg(test)]
                    {
                        bytes == TEST_PRIOR_ASSET
                    }
                    #[cfg(not(test))]
                    {
                        false
                    }
                })
    }
    const _: () = assert!(PRIOR_OFFICIAL.len() <= MAX_PRIOR_OFFICIAL);

    fn canonical_identity(dir: &Dir, name: &[u8]) -> Result<Identity, String> {
        let mut s = unsafe { std::mem::zeroed::<libc::stat>() };
        if unsafe {
            libc::fstatat(
                dir.fd(),
                name.as_ptr().cast(),
                &mut s,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            return Err("canonical lifecycle entry is absent or unreadable".to_string());
        }
        Ok(Identity {
            dev: s.st_dev,
            ino: s.st_ino,
            mode: s.st_mode,
            uid: s.st_uid,
            nlink: s.st_nlink,
            size: s.st_size,
        })
    }
    fn missing(dir: &Dir, name: &[u8]) -> bool {
        let mut s = unsafe { std::mem::zeroed::<libc::stat>() };
        (unsafe {
            libc::fstatat(
                dir.fd(),
                name.as_ptr().cast(),
                &mut s,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        }) != 0
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT)
    }
    fn checked_file(dir: &Dir, name: &[u8]) -> Result<VerifiedFile, String> {
        checked_file_with_links(dir, name, 1)
    }
    fn checked_file_with_links(
        dir: &Dir,
        name: &[u8],
        expected_links: libc::nlink_t,
    ) -> Result<VerifiedFile, String> {
        let fd = unsafe {
            libc::openat(
                dir.fd(),
                name.as_ptr().cast(),
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            )
        };
        if fd < 0 {
            return Err(format!(
                "cannot open extension safely: {}",
                std::io::Error::last_os_error()
            ));
        }
        let mut file = unsafe { File::from_raw_fd(fd) };
        let before = stat_fd(fd)?;
        let uid = unsafe { libc::geteuid() };
        if before.mode & libc::S_IFMT != libc::S_IFREG
            || before.uid != uid
            || before.mode & 0o777 != 0o600
            || before.nlink != expected_links
            || before.size < 0
            || before.size as usize > MAX_BYTES
        {
            return Err(
                "extension is modified, unsafe, or oversized; inspect it manually".to_string(),
            );
        }
        let mut bytes = Vec::with_capacity(before.size as usize);
        std::io::Read::by_ref(&mut file)
            .take((MAX_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > MAX_BYTES || stat_fd(fd)? != before || !recognized(&bytes) {
            return Err("extension is modified or unknown; inspect it manually".to_string());
        }
        Ok(VerifiedFile {
            file,
            identity: before,
            digest: digest(&bytes),
            bytes,
        })
    }
    fn revalidate_file(dir: &Dir, name: &[u8], file: &VerifiedFile) -> Result<(), String> {
        if canonical_identity(dir, name)? != file.identity
            || stat_fd(file.file.as_raw_fd())? != file.identity
        {
            return Err("extension identity changed during lifecycle operation".to_string());
        }
        #[cfg(test)]
        mutate_revalidation_target(dir, name);
        let copy = file.file.try_clone().map_err(|e| e.to_string())?;
        unsafe {
            libc::lseek(copy.as_raw_fd(), 0, libc::SEEK_SET);
        }
        let mut bytes = Vec::new();
        copy.take((MAX_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > MAX_BYTES
            || bytes != file.bytes
            || digest(&bytes) != file.digest
            || !recognized(&bytes)
        {
            Err("extension content changed during lifecycle operation".to_string())
        } else {
            Ok(())
        }
    }
    fn unlink(dir: &Dir, name: &[u8]) -> Result<(), String> {
        if unsafe { libc::unlinkat(dir.fd(), name.as_ptr().cast(), 0) } == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error().to_string())
        }
    }
    fn sync_dir(dir: &Dir) -> Result<(), String> {
        #[cfg(test)]
        if take_test_fault(TestFault::DirectorySyncEinval) {
            return Err("injected EINVAL directory sync failure".to_string());
        }
        #[cfg(test)]
        if take_test_fault(TestFault::DirectorySyncError) {
            return Err("injected directory sync failure".to_string());
        }
        if unsafe { libc::fsync(dir.fd()) } == 0 {
            Ok(())
        } else {
            Err(format!(
                "cannot sync extension directory: {}",
                std::io::Error::last_os_error()
            ))
        }
    }

    fn recognized_install_pair(dir: &Dir) -> Option<&'static str> {
        let target = checked_file_with_links(dir, TARGET, 2).ok()?;
        let temp = checked_file_with_links(dir, INSTALL_TEMP, 2).ok()?;
        if target.identity.dev != temp.identity.dev
            || target.identity.ino != temp.identity.ino
            || target.bytes != temp.bytes
        {
            return None;
        }
        Some(if target.digest == digest(ASSET) {
            "current"
        } else {
            "recognized-prior"
        })
    }
    fn status(agent: &Path) -> Result<String, String> {
        let dir = match walk_agent_dir(agent, false) {
            Ok(dir) => dir,
            Err(e) if e == "Pi extension directory is absent" => {
                return Ok("extension target: absent".to_string())
            }
            Err(_) => return Ok("extension target: unavailable".to_string()),
        };
        if dir.revalidate().is_err() {
            return Ok("extension target: unavailable".to_string());
        }
        if let Some(target) = recognized_install_pair(&dir) {
            return Ok(format!(
                "extension target: {target}; install residue: recognized pair (inspect manually after stopping Pi)"
            ));
        }
        let target = match checked_file(&dir, TARGET) {
            Ok(v) if v.digest == digest(ASSET) => "current",
            Ok(_) => "recognized-prior",
            Err(_) if missing(&dir, TARGET) => "absent",
            Err(_) => "unknown-or-modified",
        };
        let residue = if missing(&dir, INSTALL_TEMP) {
            ""
        } else {
            "; install residue: unknown-or-modified (inspect manually)"
        };
        Ok(format!("extension target: {target}{residue}"))
    }
    fn post_link_failure() -> Result<String, String> {
        Err("extension target is installed/present; install residue or durability needs manual inspection".to_string())
    }
    fn install(dir: &Dir, lock: &Lock) -> Result<String, String> {
        if !missing(dir, TARGET) {
            return Err(
                "extension target already exists; use status or inspect it manually".to_string(),
            );
        }
        if !missing(dir, INSTALL_TEMP) {
            return Err("install residue exists; inspect and remove the exact temp manually after stopping Pi".to_string());
        }
        dir.revalidate()?;
        lock.revalidate(dir)?;
        let fd = unsafe {
            libc::openat(
                dir.fd(),
                INSTALL_TEMP.as_ptr().cast(),
                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                0o600,
            )
        };
        if fd < 0 {
            return Err(format!(
                "cannot create install temp: {}",
                std::io::Error::last_os_error()
            ));
        }
        let mut temp = unsafe { File::from_raw_fd(fd) };
        temp.write_all(ASSET)
            .and_then(|_| temp.sync_all())
            .map_err(|_| "extension target is absent/not published; fixed install temp may be partial or unknown and requires manual inspection".to_string())?;
        let before = stat_fd(fd).map_err(|_| "extension target is absent/not published; fixed install temp may be partial or unknown and requires manual inspection".to_string())?;
        unsafe {
            libc::lseek(fd, 0, libc::SEEK_SET);
        }
        let mut reread = Vec::new();
        std::io::Read::by_ref(&mut temp)
            .take((MAX_BYTES + 1) as u64)
            .read_to_end(&mut reread)
            .map_err(|_| "extension target is absent/not published; fixed install temp may be partial or unknown and requires manual inspection".to_string())?;
        let uid = unsafe { libc::geteuid() };
        if reread != ASSET
            || before != stat_fd(fd).map_err(|_| "extension target is absent/not published; fixed install temp may be partial or unknown and requires manual inspection".to_string())?
            || before.mode & libc::S_IFMT != libc::S_IFREG
            || before.uid != uid
            || before.mode & 0o777 != 0o600
            || before.nlink != 1
            || before.size != ASSET.len() as libc::off_t
        {
            return Err("extension target is absent/not published; fixed install temp may be partial or unknown and requires manual inspection".to_string());
        }
        drop(temp);
        dir.revalidate().map_err(|_| "extension target is absent/not published; fixed install temp may be partial or unknown and requires manual inspection".to_string())?;
        lock.revalidate(dir).map_err(|_| "extension target is absent/not published; fixed install temp may be partial or unknown and requires manual inspection".to_string())?;
        if !missing(dir, TARGET) {
            return Err(
                "install target appeared; recognized install residue requires manual cleanup"
                    .to_string(),
            );
        }
        #[cfg(test)]
        if take_test_fault(TestFault::BeforeLinkTargetAppear) {
            write_test_unknown_target(dir);
        }
        if unsafe {
            libc::linkat(
                dir.fd(),
                INSTALL_TEMP.as_ptr().cast(),
                dir.fd(),
                TARGET.as_ptr().cast(),
                0,
            )
        } != 0
        {
            return Err("install did not replace an existing target; recognized install residue requires manual cleanup".to_string());
        }
        #[cfg(test)]
        if take_test_fault(TestFault::PostLinkTargetIdentity) {
            return post_link_failure();
        }
        let target = match canonical_identity(dir, TARGET) {
            Ok(value) => value,
            Err(_) => return post_link_failure(),
        };
        #[cfg(test)]
        if take_test_fault(TestFault::PostLinkTempIdentity) {
            return post_link_failure();
        }
        let temp_id = match canonical_identity(dir, INSTALL_TEMP) {
            Ok(value) => value,
            Err(_) => return post_link_failure(),
        };
        #[cfg(test)]
        if take_test_fault(TestFault::PostLinkPairMismatch) {
            return post_link_failure();
        }
        if target.dev != temp_id.dev || target.ino != temp_id.ino || target.nlink != 2 {
            return post_link_failure();
        }
        #[cfg(test)]
        if take_test_fault(TestFault::PostLinkResidueUnlink) {
            return post_link_failure();
        }
        if unlink(dir, INSTALL_TEMP).is_err() {
            return post_link_failure();
        }
        match sync_dir(dir) {
            Ok(()) => Ok("Pi extension installed; run /reload or restart Pi now".to_string()),
            Err(_) => Err("extension target is installed/present but durability is uncertain after directory sync failure".to_string()),
        }
    }
    #[cfg(test)]
    fn write_test_unknown_target(dir: &Dir) {
        let fd = unsafe {
            libc::openat(
                dir.fd(),
                TARGET.as_ptr().cast(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC,
                0o600,
            )
        };
        assert!(
            fd >= 0,
            "test mutation failed: {}",
            std::io::Error::last_os_error()
        );
        let mut file = unsafe { File::from_raw_fd(fd) };
        file.write_all(b"test-race-unknown").unwrap();
        file.sync_all().unwrap();
    }
    #[cfg(test)]
    fn mutate_remove_target(dir: &Dir) {
        if take_test_fault(TestFault::BeforeRemoveReplaceTarget) {
            unlink(dir, TARGET).unwrap();
            write_test_unknown_target(dir);
        } else if take_test_fault(TestFault::BeforeRemoveModifyBytes) {
            let fd = unsafe {
                libc::openat(
                    dir.fd(),
                    TARGET.as_ptr().cast(),
                    libc::O_WRONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                )
            };
            assert!(fd >= 0, "{}", std::io::Error::last_os_error());
            let mut file = unsafe { File::from_raw_fd(fd) };
            file.write_all(b"modified-before-remove").unwrap();
            file.sync_all().unwrap();
        } else if take_test_fault(TestFault::BeforeRemoveModifyMode) {
            assert_eq!(
                unsafe { libc::fchmodat(dir.fd(), TARGET.as_ptr().cast(), 0o644, 0) },
                0
            );
        }
    }
    #[cfg(test)]
    fn mutate_revalidation_target(dir: &Dir, name: &[u8]) {
        let fault = [
            TestFault::BeforeRevalidateGrow,
            TestFault::BeforeRevalidateTruncate,
            TestFault::BeforeRevalidateReplaceContent,
        ]
        .into_iter()
        .find(|fault| take_test_fault(*fault));
        let Some(fault) = fault else { return };
        let fd = unsafe {
            libc::openat(
                dir.fd(),
                name.as_ptr().cast(),
                libc::O_WRONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        assert!(fd >= 0, "{}", std::io::Error::last_os_error());
        let mut file = unsafe { File::from_raw_fd(fd) };
        match fault {
            TestFault::BeforeRevalidateGrow => {
                assert_eq!(
                    unsafe { libc::ftruncate(fd, (MAX_BYTES + 1) as libc::off_t) },
                    0
                );
            }
            TestFault::BeforeRevalidateTruncate => {
                assert_eq!(unsafe { libc::ftruncate(fd, 1) }, 0);
            }
            TestFault::BeforeRevalidateReplaceContent => {
                file.write_all(&vec![b'x'; ASSET.len()]).unwrap();
                file.sync_all().unwrap();
            }
            _ => unreachable!(),
        }
    }

    fn remove(dir: &Dir, lock: &Lock) -> Result<String, String> {
        let target = checked_file(dir, TARGET).map_err(|e| {
            if missing(dir, TARGET) {
                "extension target is absent; not installed".to_string()
            } else {
                e
            }
        })?;
        dir.revalidate()?;
        lock.revalidate(dir)?;
        #[cfg(test)]
        mutate_remove_target(dir);
        revalidate_file(dir, TARGET, &target)?;
        dir.revalidate()?;
        lock.revalidate(dir)?;
        unlink(dir, TARGET)?;
        match sync_dir(dir) {
            Ok(()) => Ok("Pi extension removed; run /reload or restart Pi now".to_string()),
            Err(_) => Err(
                "extension removed but durability is uncertain after directory sync failure"
                    .to_string(),
            ),
        }
    }

    struct Lock(File);
    impl Lock {
        fn acquire(dir: &Dir) -> Result<Self, String> {
            let fd = unsafe {
                libc::openat(
                    dir.fd(),
                    LOCK.as_ptr().cast(),
                    libc::O_RDWR | libc::O_CREAT | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                    0o600,
                )
            };
            if fd < 0 {
                return Err(format!(
                    "cannot open lifecycle lock: {}",
                    std::io::Error::last_os_error()
                ));
            }
            let file = unsafe { File::from_raw_fd(fd) };
            validate_lock(fd)?;
            let deadline = Instant::now() + Duration::from_secs(1);
            loop {
                if unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                    return Ok(Self(file));
                }
                if Instant::now() >= deadline {
                    return Err("extension lifecycle command is busy; try again".to_string());
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        fn revalidate(&self, dir: &Dir) -> Result<(), String> {
            validate_lock(self.0.as_raw_fd())?;
            if canonical_identity(dir, LOCK)? != stat_fd(self.0.as_raw_fd())? {
                Err("lifecycle lock was replaced".to_string())
            } else {
                Ok(())
            }
        }
    }
    fn validate_lock(fd: RawFd) -> Result<(), String> {
        let s = stat_fd(fd)?;
        let uid = unsafe { libc::geteuid() };
        if s.mode & libc::S_IFMT != libc::S_IFREG
            || s.uid != uid
            || s.mode & 0o777 != 0o600
            || s.nlink != 1
            || s.size > MAX_BYTES as libc::off_t
        {
            Err("unsafe lifecycle lock".to_string())
        } else {
            Ok(())
        }
    }
    impl Drop for Lock {
        fn drop(&mut self) {
            unsafe {
                libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::collections::BTreeMap;
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        use std::sync::mpsc;

        #[test]
        fn resolver_accepts_only_exact_tilde_forms() {
            let home = Path::new("/home/test");
            assert_eq!(
                effective_agent_dir(home, Some(OsString::from("~/x"))).unwrap(),
                PathBuf::from("/home/test/x")
            );
            for value in ["~//tmp", "~user", "a~b", "relative"] {
                assert!(effective_agent_dir(home, Some(OsString::from(value))).is_err());
            }
        }
        fn sandbox() -> tempfile::TempDir {
            tempfile::Builder::new()
                .prefix("ptop-lifecycle-test-")
                .tempdir_in(std::env::current_dir().unwrap())
                .unwrap()
        }

        #[test]
        fn status_is_read_only_for_an_absent_directory() {
            let root = sandbox();
            let agent = root.path().join("agent");
            assert_eq!(status(&agent).unwrap(), "extension target: absent");
            assert!(!agent.exists());
        }

        #[test]
        fn install_and_remove_only_accept_recognized_content() {
            let root = sandbox();
            let agent = root.path().join("agent");
            let dir = walk_agent_dir(&agent, true).unwrap();
            let lock = Lock::acquire(&dir).unwrap();
            assert!(install(&dir, &lock).unwrap().contains("installed"));
            assert!(status(&agent).unwrap().contains("current"));
            assert!(remove(&dir, &lock).unwrap().contains("removed"));
            assert_eq!(status(&agent).unwrap(), "extension target: absent");
        }

        #[test]
        fn status_classifies_only_the_exact_recognized_interrupted_pair() {
            let root = sandbox();
            let agent = root.path().join("agent");
            let dir = walk_agent_dir(&agent, true).unwrap();
            let fd = unsafe {
                libc::openat(
                    dir.fd(),
                    TARGET.as_ptr().cast(),
                    libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC,
                    0o600,
                )
            };
            let mut file = unsafe { File::from_raw_fd(fd) };
            file.write_all(ASSET).unwrap();
            file.sync_all().unwrap();
            drop(file);
            assert_eq!(
                unsafe {
                    libc::linkat(
                        dir.fd(),
                        TARGET.as_ptr().cast(),
                        dir.fd(),
                        INSTALL_TEMP.as_ptr().cast(),
                        0,
                    )
                },
                0
            );
            assert!(status(&agent).unwrap().contains("recognized pair"));
            assert!(
                checked_file(&dir, TARGET).is_err(),
                "pair must not authorize removal"
            );
            unlink(&dir, INSTALL_TEMP).unwrap();
            assert!(status(&agent).unwrap().contains("current"));
        }

        #[test]
        fn install_maps_directory_sync_failure_to_durability_uncertainty() {
            let root = sandbox();
            let agent = root.path().join("agent");
            let dir = walk_agent_dir(&agent, true).unwrap();
            let lock = Lock::acquire(&dir).unwrap();
            TEST_FAULT.with(|fault| fault.set(Some(TestFault::DirectorySyncEinval)));
            let result = install(&dir, &lock).unwrap_err();
            assert!(result.contains("installed/present but durability is uncertain"));
            assert!(!missing(&dir, TARGET));
            assert!(
                TEST_FAULT.with(|fault| fault.get().is_none()),
                "fault seam was not reached"
            );
        }

        fn write_entry(dir: &Dir, name: &[u8], bytes: &[u8], mode: u32) {
            let fd = unsafe {
                libc::openat(
                    dir.fd(),
                    name.as_ptr().cast(),
                    libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC,
                    mode,
                )
            };
            assert!(fd >= 0, "{}", std::io::Error::last_os_error());
            let mut file = unsafe { File::from_raw_fd(fd) };
            file.write_all(bytes).unwrap();
            file.sync_all().unwrap();
        }

        #[test]
        fn post_link_fault_matrix_preserves_the_recognized_pair() {
            for fault in [
                TestFault::PostLinkTargetIdentity,
                TestFault::PostLinkTempIdentity,
                TestFault::PostLinkPairMismatch,
                TestFault::PostLinkResidueUnlink,
            ] {
                let root = sandbox();
                let agent = root.path().join("agent");
                let dir = walk_agent_dir(&agent, true).unwrap();
                let lock = Lock::acquire(&dir).unwrap();
                TEST_FAULT.with(|value| value.set(Some(fault)));
                let error = install(&dir, &lock).unwrap_err();
                assert!(error.contains("installed/present"), "{fault:?}: {error}");
                assert!(!missing(&dir, TARGET), "{fault:?}");
                assert!(!missing(&dir, INSTALL_TEMP), "{fault:?}");
                assert!(status(&agent).unwrap().contains("recognized pair"));
                assert!(
                    TEST_FAULT.with(|value| value.get().is_none()),
                    "{fault:?} seam was not reached"
                );
            }
        }

        #[test]
        fn malformed_two_link_pairs_are_never_recognized() {
            for variant in ["bytes", "mode", "inode"] {
                let root = sandbox();
                let agent = root.path().join("agent");
                let dir = walk_agent_dir(&agent, true).unwrap();
                write_entry(&dir, TARGET, ASSET, 0o600);
                if variant == "inode" {
                    write_entry(&dir, INSTALL_TEMP, ASSET, 0o600);
                } else {
                    assert_eq!(
                        unsafe {
                            libc::linkat(
                                dir.fd(),
                                TARGET.as_ptr().cast(),
                                dir.fd(),
                                INSTALL_TEMP.as_ptr().cast(),
                                0,
                            )
                        },
                        0
                    );
                    if variant == "bytes" {
                        let fd = unsafe {
                            libc::openat(
                                dir.fd(),
                                TARGET.as_ptr().cast(),
                                libc::O_WRONLY | libc::O_CLOEXEC,
                                0,
                            )
                        };
                        assert!(fd >= 0);
                        assert_eq!(unsafe { libc::ftruncate(fd, 1) }, 0);
                        unsafe { libc::close(fd) };
                    } else {
                        assert_eq!(
                            unsafe { libc::fchmodat(dir.fd(), TARGET.as_ptr().cast(), 0o644, 0) },
                            0
                        );
                    }
                }
                assert!(
                    !status(&agent).unwrap().contains("recognized pair"),
                    "{variant}"
                );
            }
        }

        #[test]
        fn partial_and_complete_temps_are_never_cleaned_by_lifecycle_commands() {
            for bytes in [&ASSET[..1], ASSET] {
                let root = sandbox();
                let agent = root.path().join("agent");
                let dir = walk_agent_dir(&agent, true).unwrap();
                write_entry(&dir, INSTALL_TEMP, bytes, 0o600);
                assert!(status(&agent).unwrap().contains("install residue"));
                let lock = Lock::acquire(&dir).unwrap();
                assert!(install(&dir, &lock).is_err());
                assert!(remove(&dir, &lock).is_err());
                assert!(!missing(&dir, INSTALL_TEMP));
            }
        }

        #[test]
        fn install_does_not_replace_a_target_that_appears_before_linkat() {
            let root = sandbox();
            let agent = root.path().join("agent");
            let dir = walk_agent_dir(&agent, true).unwrap();
            let lock = Lock::acquire(&dir).unwrap();
            TEST_FAULT.with(|fault| fault.set(Some(TestFault::BeforeLinkTargetAppear)));
            let error = install(&dir, &lock).unwrap_err();
            assert!(error.contains("did not replace"));
            let target = match checked_file(&dir, TARGET) {
                Ok(_) => panic!("race target was incorrectly recognized"),
                Err(error) => error,
            };
            assert!(target.contains("unknown") || target.contains("modified"));
            assert_eq!(
                std::fs::read(agent.join("extensions/ptop-live-harness.ts")).unwrap(),
                b"test-race-unknown"
            );
            assert_eq!(
                std::fs::read(agent.join("extensions/.ptop-live-install")).unwrap(),
                ASSET
            );
        }

        #[test]
        fn remove_sync_faults_leave_absent_target_with_explicit_uncertainty() {
            for fault in [
                TestFault::DirectorySyncEinval,
                TestFault::DirectorySyncError,
            ] {
                let root = sandbox();
                let agent = root.path().join("agent");
                let dir = walk_agent_dir(&agent, true).unwrap();
                let lock = Lock::acquire(&dir).unwrap();
                install(&dir, &lock).unwrap();
                TEST_FAULT.with(|value| value.set(Some(fault)));
                let error = remove(&dir, &lock).unwrap_err();
                assert!(
                    error.contains("removed but durability is uncertain"),
                    "{fault:?}"
                );
                assert!(missing(&dir, TARGET), "{fault:?}");
                assert!(TEST_FAULT.with(|value| value.get().is_none()));
            }
        }

        #[test]
        fn remove_accepts_current_and_injected_recognized_prior_assets() {
            for bytes in [ASSET, TEST_PRIOR_ASSET] {
                let root = sandbox();
                let agent = root.path().join("agent");
                let dir = walk_agent_dir(&agent, true).unwrap();
                write_entry(&dir, TARGET, bytes, 0o600);
                let lock = Lock::acquire(&dir).unwrap();
                assert!(remove(&dir, &lock).unwrap().contains("removed"));
                assert!(missing(&dir, TARGET));
            }
        }

        #[test]
        fn remove_refusal_matrix_preserves_every_observed_target() {
            for variant in [
                "absent",
                "unknown",
                "oversized",
                "symlink",
                "directory",
                "fifo",
                "mode",
                "hard-link",
                "modified-recognized",
            ] {
                let root = sandbox();
                let agent = root.path().join("agent");
                let dir = walk_agent_dir(&agent, true).unwrap();
                match variant {
                    "absent" => {}
                    "unknown" => write_entry(&dir, TARGET, b"unknown", 0o600),
                    "oversized" => write_entry(&dir, TARGET, &vec![0; MAX_BYTES + 1], 0o600),
                    "symlink" => std::os::unix::fs::symlink(
                        "elsewhere",
                        agent.join("extensions/ptop-live-harness.ts"),
                    )
                    .unwrap(),
                    "mode" => write_entry(&dir, TARGET, ASSET, 0o644),
                    "hard-link" => {
                        write_entry(&dir, TARGET, ASSET, 0o600);
                        let other = b"other\0";
                        assert_eq!(
                            unsafe {
                                libc::linkat(
                                    dir.fd(),
                                    TARGET.as_ptr().cast(),
                                    dir.fd(),
                                    other.as_ptr().cast(),
                                    0,
                                )
                            },
                            0
                        );
                    }
                    "directory" => assert_eq!(
                        unsafe { libc::mkdirat(dir.fd(), TARGET.as_ptr().cast(), 0o700) },
                        0
                    ),
                    "fifo" => assert_eq!(
                        unsafe { libc::mkfifoat(dir.fd(), TARGET.as_ptr().cast(), 0o600) },
                        0
                    ),
                    "modified-recognized" => {
                        let mut modified = ASSET.to_vec();
                        modified[0] ^= 1;
                        write_entry(&dir, TARGET, &modified, 0o600);
                    }
                    _ => unreachable!(),
                }
                let target_path = agent.join("extensions/ptop-live-harness.ts");
                let before = if variant == "absent" {
                    None
                } else {
                    Some((
                        std::fs::symlink_metadata(&target_path).unwrap(),
                        if variant == "symlink" {
                            Some(std::fs::read_link(&target_path).unwrap())
                        } else {
                            None
                        },
                        if matches!(
                            variant,
                            "unknown" | "oversized" | "mode" | "hard-link" | "modified-recognized"
                        ) {
                            Some(std::fs::read(&target_path).unwrap())
                        } else {
                            None
                        },
                    ))
                };
                let lock = Lock::acquire(&dir).unwrap();
                assert!(remove(&dir, &lock).is_err(), "{variant}");
                if let Some((before_meta, before_link, before_bytes)) = before {
                    let after_meta = std::fs::symlink_metadata(&target_path).unwrap();
                    assert_eq!(
                        (
                            after_meta.mode(),
                            after_meta.nlink(),
                            after_meta.dev(),
                            after_meta.ino()
                        ),
                        (
                            before_meta.mode(),
                            before_meta.nlink(),
                            before_meta.dev(),
                            before_meta.ino()
                        ),
                        "{variant}"
                    );
                    assert_eq!(
                        before_link,
                        if variant == "symlink" {
                            Some(std::fs::read_link(&target_path).unwrap())
                        } else {
                            None
                        }
                    );
                    assert_eq!(
                        before_bytes,
                        if matches!(
                            variant,
                            "unknown" | "oversized" | "mode" | "hard-link" | "modified-recognized"
                        ) {
                            Some(std::fs::read(&target_path).unwrap())
                        } else {
                            None
                        }
                    );
                } else {
                    assert!(!target_path.exists(), "{variant}");
                }
            }
        }

        #[test]
        fn remove_revalidation_races_preserve_replacement_and_mutations() {
            for (fault, expected_bytes, expected_mode) in [
                (
                    TestFault::BeforeRemoveReplaceTarget,
                    Some(b"test-race-unknown".as_slice()),
                    0o600,
                ),
                (TestFault::BeforeRemoveModifyBytes, None, 0o600),
                (TestFault::BeforeRemoveModifyMode, Some(ASSET), 0o644),
            ] {
                let root = sandbox();
                let agent = root.path().join("agent");
                let dir = walk_agent_dir(&agent, true).unwrap();
                let lock = Lock::acquire(&dir).unwrap();
                install(&dir, &lock).unwrap();
                let before = canonical_identity(&dir, TARGET).unwrap();
                TEST_FAULT.with(|value| value.set(Some(fault)));
                let error = remove(&dir, &lock).unwrap_err();
                assert!(error.contains("changed"), "{fault:?}: {error}");
                let after = canonical_identity(&dir, TARGET).unwrap();
                if fault == TestFault::BeforeRemoveReplaceTarget {
                    assert_ne!(after.ino, before.ino);
                }
                assert_eq!(after.mode & 0o777, expected_mode, "{fault:?}");
                if let Some(bytes) = expected_bytes {
                    assert_eq!(
                        std::fs::read(agent.join("extensions/ptop-live-harness.ts")).unwrap(),
                        bytes
                    );
                } else {
                    assert!(std::fs::read(agent.join("extensions/ptop-live-harness.ts"))
                        .unwrap()
                        .starts_with(b"modified-before-remove"));
                }
                assert!(TEST_FAULT.with(|value| value.get().is_none()));
            }
        }

        #[test]
        fn bounded_revalidation_rejects_growth_truncation_and_replaced_content() {
            for fault in [
                TestFault::BeforeRevalidateGrow,
                TestFault::BeforeRevalidateTruncate,
                TestFault::BeforeRevalidateReplaceContent,
            ] {
                let root = sandbox();
                let agent = root.path().join("agent");
                let dir = walk_agent_dir(&agent, true).unwrap();
                let lock = Lock::acquire(&dir).unwrap();
                install(&dir, &lock).unwrap();
                TEST_FAULT.with(|value| value.set(Some(fault)));
                let error = remove(&dir, &lock).unwrap_err();
                assert!(error.contains("content changed"), "{fault:?}: {error}");
                let metadata =
                    std::fs::metadata(agent.join("extensions/ptop-live-harness.ts")).unwrap();
                assert!(metadata.is_file(), "{fault:?}");
                match fault {
                    TestFault::BeforeRevalidateGrow => {
                        assert_eq!(metadata.len(), (MAX_BYTES + 1) as u64)
                    }
                    TestFault::BeforeRevalidateTruncate => assert_eq!(metadata.len(), 1),
                    TestFault::BeforeRevalidateReplaceContent => {
                        assert_eq!(metadata.len(), ASSET.len() as u64)
                    }
                    _ => unreachable!(),
                }
                assert!(TEST_FAULT.with(|value| value.get().is_none()));
            }
        }

        #[test]
        fn mkdir_sync_failure_does_not_create_target_or_lock_below_new_directory() {
            let root = sandbox();
            let agent = root.path().join("anchor/new-agent");
            std::fs::create_dir(root.path().join("anchor")).unwrap();
            TEST_FAULT.with(|fault| fault.set(Some(TestFault::DirectorySyncEinval)));
            let error = match walk_agent_dir(&agent, true) {
                Ok(_) => panic!("directory walk unexpectedly succeeded"),
                Err(error) => error,
            };
            assert!(error.contains("durability is uncertain"));
            assert!(!agent
                .join("extensions")
                .join(OsStr::from_bytes(LOCK))
                .exists());
            assert!(!agent
                .join("extensions")
                .join(OsStr::from_bytes(TARGET))
                .exists());
        }

        #[test]
        fn path_walk_accepts_safe_injected_root_owned_chain_and_requires_user_anchor_to_create() {
            let root = sandbox();
            let existing = root.path().join("existing");
            std::fs::create_dir(&existing).unwrap();
            std::fs::create_dir(existing.join("extensions")).unwrap();
            TEST_ALL_DIRECTORY_OWNERS_ROOT.with(|enabled| enabled.set(true));
            assert!(walk_agent_dir(&existing, false).is_ok());
            let absent = root.path().join("no-user-anchor");
            let error = match walk_agent_dir(&absent, true) {
                Ok(_) => panic!("walk unexpectedly found a user-owned creation anchor"),
                Err(error) => error,
            };
            assert_eq!(error, "Pi extension directory is absent");
            assert!(!absent.exists());
            TEST_ALL_DIRECTORY_OWNERS_ROOT.with(|enabled| enabled.set(false));
        }

        #[test]
        fn path_walk_rejects_unsafe_symlink_and_non_directory_components() {
            for variant in ["group-writable", "world-writable", "symlink", "file"] {
                let root = sandbox();
                let component = root.path().join("component");
                match variant {
                    "group-writable" | "world-writable" => {
                        std::fs::create_dir(&component).unwrap();
                        let mode = if variant == "group-writable" {
                            0o770
                        } else {
                            0o777
                        };
                        std::fs::set_permissions(&component, std::fs::Permissions::from_mode(mode))
                            .unwrap();
                    }
                    "symlink" => std::os::unix::fs::symlink(root.path(), &component).unwrap(),
                    "file" => std::fs::write(&component, b"not a directory").unwrap(),
                    _ => unreachable!(),
                }
                assert!(
                    walk_agent_dir(&component.join("agent"), false).is_err(),
                    "{variant}"
                );
            }
        }

        #[test]
        fn directory_revalidation_rejects_renamed_or_mode_changed_component() {
            let root = sandbox();
            let agent = root.path().join("anchor/middle/agent");
            std::fs::create_dir(root.path().join("anchor")).unwrap();
            std::fs::create_dir_all(agent.join("extensions")).unwrap();
            let directory = walk_agent_dir(&agent, false).unwrap();
            let middle = root.path().join("anchor/middle");
            std::fs::rename(&middle, root.path().join("anchor/old-middle")).unwrap();
            std::fs::create_dir_all(agent.join("extensions")).unwrap();
            assert!(directory.revalidate().is_err());

            let directory = walk_agent_dir(&agent, false).unwrap();
            std::fs::set_permissions(&middle, std::fs::Permissions::from_mode(0o700)).unwrap();
            assert!(directory.revalidate().is_err());
        }

        fn snapshot_tree(root: &Path) -> BTreeMap<PathBuf, (u32, u64, u64, u64, Vec<u8>)> {
            fn visit(
                root: &Path,
                path: &Path,
                entries: &mut BTreeMap<PathBuf, (u32, u64, u64, u64, Vec<u8>)>,
            ) {
                for entry in std::fs::read_dir(path).unwrap() {
                    let entry = entry.unwrap();
                    let path = entry.path();
                    let metadata = std::fs::symlink_metadata(&path).unwrap();
                    let bytes = if metadata.file_type().is_file() {
                        std::fs::read(&path).unwrap()
                    } else {
                        Vec::new()
                    };
                    entries.insert(
                        path.strip_prefix(root).unwrap().to_path_buf(),
                        (
                            metadata.mode(),
                            metadata.nlink(),
                            metadata.dev(),
                            metadata.ino(),
                            bytes,
                        ),
                    );
                    if metadata.file_type().is_dir() {
                        visit(root, &path, entries);
                    }
                }
            }
            let mut entries = BTreeMap::new();
            visit(root, root, &mut entries);
            entries
        }

        #[test]
        fn status_is_read_only_for_every_existing_target_and_residue_case() {
            for variant in [
                "absent",
                "current",
                "unknown",
                "partial-temp",
                "complete-temp",
                "pair",
            ] {
                let root = sandbox();
                let agent = root.path().join("agent");
                let dir = walk_agent_dir(&agent, true).unwrap();
                match variant {
                    "current" => write_entry(&dir, TARGET, ASSET, 0o600),
                    "unknown" => write_entry(&dir, TARGET, b"unknown", 0o600),
                    "partial-temp" => write_entry(&dir, INSTALL_TEMP, &ASSET[..1], 0o600),
                    "complete-temp" => write_entry(&dir, INSTALL_TEMP, ASSET, 0o600),
                    "pair" => {
                        write_entry(&dir, TARGET, ASSET, 0o600);
                        assert_eq!(
                            unsafe {
                                libc::linkat(
                                    dir.fd(),
                                    TARGET.as_ptr().cast(),
                                    dir.fd(),
                                    INSTALL_TEMP.as_ptr().cast(),
                                    0,
                                )
                            },
                            0
                        );
                    }
                    "absent" => {}
                    _ => unreachable!(),
                }
                let before = snapshot_tree(&agent);
                let result = status(&agent).unwrap();
                let after = snapshot_tree(&agent);
                assert_eq!(after, before, "{variant}: {result}");
                assert!(
                    !agent
                        .join("extensions")
                        .join(OsStr::from_bytes(&LOCK[..LOCK.len() - 1]))
                        .exists(),
                    "{variant}"
                );
            }
        }

        #[test]
        fn lock_rejects_unsafe_existing_entries_without_removing_them() {
            for variant in ["mode", "symlink", "directory", "fifo", "hard-link"] {
                let root = sandbox();
                let agent = root.path().join("agent");
                let dir = walk_agent_dir(&agent, true).unwrap();
                let lock_path = agent
                    .join("extensions")
                    .join(OsStr::from_bytes(&LOCK[..LOCK.len() - 1]));
                match variant {
                    "mode" => {
                        write_entry(&dir, LOCK, b"", 0o600);
                        std::fs::set_permissions(
                            &lock_path,
                            std::fs::Permissions::from_mode(0o644),
                        )
                        .unwrap();
                    }
                    "symlink" => std::os::unix::fs::symlink("elsewhere", &lock_path).unwrap(),
                    "directory" => std::fs::create_dir(&lock_path).unwrap(),
                    "fifo" => assert_eq!(
                        unsafe { libc::mkfifoat(dir.fd(), LOCK.as_ptr().cast(), 0o600) },
                        0
                    ),
                    "hard-link" => {
                        write_entry(&dir, LOCK, b"", 0o600);
                        let copy = c_name(OsStr::new("lock-copy")).unwrap();
                        assert_eq!(
                            unsafe {
                                libc::linkat(
                                    dir.fd(),
                                    LOCK.as_ptr().cast(),
                                    dir.fd(),
                                    copy.as_ptr().cast(),
                                    0,
                                )
                            },
                            0
                        );
                    }
                    _ => unreachable!(),
                }
                let before = std::fs::symlink_metadata(&lock_path).unwrap();
                assert!(Lock::acquire(&dir).is_err(), "{variant}");
                let after = std::fs::symlink_metadata(&lock_path).unwrap();
                assert_eq!(
                    (after.mode(), after.nlink(), after.dev(), after.ino()),
                    (before.mode(), before.nlink(), before.dev(), before.ino()),
                    "{variant}"
                );
            }
        }

        #[test]
        fn lock_revalidation_rejects_a_replaced_canonical_path() {
            let root = sandbox();
            let agent = root.path().join("agent");
            let dir = walk_agent_dir(&agent, true).unwrap();
            let lock = Lock::acquire(&dir).unwrap();
            unlink(&dir, LOCK).unwrap();
            write_entry(&dir, LOCK, b"", 0o600);
            assert!(lock.revalidate(&dir).is_err());
        }

        #[test]
        fn second_lock_attempt_is_bounded_then_succeeds_after_release() {
            let root = sandbox();
            let agent = root.path().join("agent");
            let dir = walk_agent_dir(&agent, true).unwrap();
            let lock = Lock::acquire(&dir).unwrap();
            let (start_tx, start_rx) = mpsc::channel();
            let (result_tx, result_rx) = mpsc::channel();
            let worker_agent = agent.clone();
            let worker = std::thread::spawn(move || {
                start_rx.recv().unwrap();
                let first = Lock::acquire(&walk_agent_dir(&worker_agent, false).unwrap());
                result_tx.send(first.map(|_| ())).unwrap();
                start_rx.recv().unwrap();
                let second = Lock::acquire(&walk_agent_dir(&worker_agent, false).unwrap());
                result_tx.send(second.map(|_| ())).unwrap();
            });
            let started = Instant::now();
            start_tx.send(()).unwrap();
            let first = result_rx.recv().unwrap();
            assert!(first.unwrap_err().contains("busy"));
            assert!(started.elapsed() <= Duration::from_millis(1100));
            drop(lock);
            start_tx.send(()).unwrap();
            assert!(result_rx.recv().unwrap().is_ok());
            worker.join().unwrap();
        }

        #[test]
        fn concurrent_modifying_lock_gates_admit_only_one_while_held() {
            let root = sandbox();
            let agent = root.path().join("agent");
            walk_agent_dir(&agent, true).unwrap();
            let (holder_tx, holder_rx) = mpsc::channel();
            let (release_tx, release_rx) = mpsc::channel();
            let holder_agent = agent.clone();
            let holder = std::thread::spawn(move || {
                let dir = walk_agent_dir(&holder_agent, false).unwrap();
                let lock = Lock::acquire(&dir).unwrap();
                // This is the same lock and revalidation gate used before mutation.
                lock.revalidate(&dir).unwrap();
                holder_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                drop(lock);
            });
            holder_rx.recv().unwrap();
            let (contender_tx, contender_rx) = mpsc::channel();
            let contender = std::thread::spawn(move || {
                let dir = walk_agent_dir(&agent, false).unwrap();
                let admitted = Lock::acquire(&dir)
                    .and_then(|lock| lock.revalidate(&dir).map(|_| lock))
                    .is_ok();
                contender_tx.send(admitted).unwrap();
            });
            assert!(!contender_rx.recv().unwrap());
            release_tx.send(()).unwrap();
            holder.join().unwrap();
            contender.join().unwrap();
        }

        #[test]
        fn allowlist_has_exact_prior_capacity() {
            assert_eq!(PRIOR_OFFICIAL.len(), 0);
            assert!(!recognized(&vec![0; MAX_BYTES + 1]));
        }
    }
}
#[cfg(unix)]
pub fn execute(command: Command) -> Result<String, String> {
    unix::execute(command)
}
