//! Filesystem-backed SFTP subsystem for the dev server. The SFTP root `/`
//! maps to a single local directory; paths can never escape it.

use russh_sftp::protocol::{
    Attrs, Data, File, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode, Version,
};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

enum Open {
    File(std::fs::File),
    /// Directory listing; `None` once it has been sent.
    Dir(Option<Vec<File>>),
}

pub struct SftpFs {
    root: PathBuf,
    handles: HashMap<String, Open>,
    next_handle: u64,
}

impl SftpFs {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            handles: HashMap::new(),
            next_handle: 0,
        }
    }

    /// Normalize an SFTP path to `/a/b` form (no `.`/`..`, never above `/`).
    fn normalize(path: &str) -> String {
        let mut parts: Vec<&str> = Vec::new();
        for part in path.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    parts.pop();
                }
                p => parts.push(p),
            }
        }
        format!("/{}", parts.join("/"))
    }

    fn local(&self, path: &str) -> PathBuf {
        let norm = Self::normalize(path);
        let mut out = self.root.clone();
        for part in norm.split('/').filter(|p| !p.is_empty()) {
            out.push(part);
        }
        out
    }

    fn handle(&mut self, open: Open) -> String {
        self.next_handle += 1;
        let h = self.next_handle.to_string();
        self.handles.insert(h.clone(), open);
        h
    }
}

fn ok(id: u32) -> Status {
    Status {
        id,
        status_code: StatusCode::Ok,
        error_message: "Ok".into(),
        language_tag: "en-US".into(),
    }
}

fn io_err(e: std::io::Error) -> StatusCode {
    match e.kind() {
        std::io::ErrorKind::NotFound => StatusCode::NoSuchFile,
        std::io::ErrorKind::PermissionDenied => StatusCode::PermissionDenied,
        _ => StatusCode::Failure,
    }
}

fn attrs(path: &Path) -> Result<FileAttributes, StatusCode> {
    std::fs::metadata(path)
        .map(|m| FileAttributes::from(&m))
        .map_err(io_err)
}

impl russh_sftp::server::Handler for SftpFs {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn init(
        &mut self,
        _version: u32,
        _ext: HashMap<String, String>,
    ) -> Result<Version, Self::Error> {
        Ok(Version::new())
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        Ok(Name {
            id,
            files: vec![File::dummy(Self::normalize(&path))],
        })
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        Ok(Attrs {
            id,
            attrs: attrs(&self.local(&path))?,
        })
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        self.stat(id, path).await
    }

    async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, Self::Error> {
        match self.handles.get(&handle) {
            Some(Open::File(f)) => Ok(Attrs {
                id,
                attrs: f
                    .metadata()
                    .map(|m| FileAttributes::from(&m))
                    .map_err(io_err)?,
            }),
            _ => Err(StatusCode::Failure),
        }
    }

    async fn setstat(
        &mut self,
        id: u32,
        path: String,
        attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        if let Some(mode) = attrs.permissions {
            let local = self.local(&path);
            let mut perms = std::fs::metadata(&local).map_err(io_err)?.permissions();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                perms.set_mode(mode & 0o7777);
            }
            // Windows only has "read-only": no write bit for the owner.
            #[cfg(not(unix))]
            perms.set_readonly(mode & 0o200 == 0);
            std::fs::set_permissions(&local, perms).map_err(io_err)?;
        }
        Ok(ok(id))
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        let dir = self.local(&path);
        let mut files = Vec::new();
        for entry in std::fs::read_dir(&dir).map_err(io_err)? {
            let entry = entry.map_err(io_err)?;
            let Ok(meta) = entry.metadata() else { continue };
            files.push(File::new(
                entry.file_name().to_string_lossy().into_owned(),
                FileAttributes::from(&meta),
            ));
        }
        Ok(Handle {
            id,
            handle: self.handle(Open::Dir(Some(files))),
        })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        match self.handles.get_mut(&handle) {
            Some(Open::Dir(files)) => match files.take() {
                Some(files) => Ok(Name { id, files }),
                None => Err(StatusCode::Eof),
            },
            _ => Err(StatusCode::Failure),
        }
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        pflags: OpenFlags,
        _attrs: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        let file = std::fs::OpenOptions::new()
            .read(pflags.contains(OpenFlags::READ))
            .write(pflags.contains(OpenFlags::WRITE) || pflags.contains(OpenFlags::APPEND))
            .append(pflags.contains(OpenFlags::APPEND))
            .create(pflags.contains(OpenFlags::CREATE))
            .truncate(pflags.contains(OpenFlags::TRUNCATE))
            .open(self.local(&filename))
            .map_err(io_err)?;
        Ok(Handle {
            id,
            handle: self.handle(Open::File(file)),
        })
    }

    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<Data, Self::Error> {
        let Some(Open::File(f)) = self.handles.get_mut(&handle) else {
            return Err(StatusCode::Failure);
        };
        f.seek(SeekFrom::Start(offset)).map_err(io_err)?;
        let mut data = vec![0; len.min(256 * 1024) as usize];
        let n = f.read(&mut data).map_err(io_err)?;
        if n == 0 {
            return Err(StatusCode::Eof);
        }
        data.truncate(n);
        Ok(Data { id, data })
    }

    async fn write(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<Status, Self::Error> {
        let Some(Open::File(f)) = self.handles.get_mut(&handle) else {
            return Err(StatusCode::Failure);
        };
        f.seek(SeekFrom::Start(offset)).map_err(io_err)?;
        f.write_all(&data).map_err(io_err)?;
        Ok(ok(id))
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        self.handles.remove(&handle);
        Ok(ok(id))
    }

    async fn remove(&mut self, id: u32, filename: String) -> Result<Status, Self::Error> {
        std::fs::remove_file(self.local(&filename)).map_err(io_err)?;
        Ok(ok(id))
    }

    async fn mkdir(
        &mut self,
        id: u32,
        path: String,
        _attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        std::fs::create_dir(self.local(&path)).map_err(io_err)?;
        Ok(ok(id))
    }

    async fn rmdir(&mut self, id: u32, path: String) -> Result<Status, Self::Error> {
        std::fs::remove_dir(self.local(&path)).map_err(io_err)?;
        Ok(ok(id))
    }

    async fn rename(
        &mut self,
        id: u32,
        oldpath: String,
        newpath: String,
    ) -> Result<Status, Self::Error> {
        std::fs::rename(self.local(&oldpath), self.local(&newpath)).map_err(io_err)?;
        Ok(ok(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_never_escape_root() {
        assert_eq!(SftpFs::normalize("/a/./b/../c"), "/a/c");
        assert_eq!(SftpFs::normalize("../../etc/passwd"), "/etc/passwd");
        assert_eq!(SftpFs::normalize(""), "/");
        let fs = SftpFs::new(PathBuf::from("root"));
        assert_eq!(fs.local("/../../x"), PathBuf::from("root").join("x"));
    }
}
