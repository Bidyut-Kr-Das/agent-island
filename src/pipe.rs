use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use windows::Win32::Foundation::{ERROR_PIPE_CONNECTED, GetLastError, HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeServerProcessId, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT, WaitNamedPipeW,
};
use windows::core::{HSTRING, w};

const ERROR_FILE_NOT_FOUND: i32 = 2;
const ERROR_PIPE_BUSY: i32 = 231;

pub fn pipe_name() -> String {
    format!(r"\\.\pipe\agent-islands-{}", std::env::var("USERNAME").unwrap_or_default())
}

/// Owns the not-yet-connected next pipe instance.
pub struct Server {
    name: String,
    pending: OwnedHandle,
}

fn create_instance(name: &str, first: bool) -> io::Result<OwnedHandle> {
    let mut sd = PSECURITY_DESCRIPTOR::default();
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            w!("D:P(A;;GA;;;OW)"),
            SDDL_REVISION_1,
            &mut sd,
            None,
        )?;
    }
    let sa = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.0,
        bInheritHandle: false.into(),
    };
    let mut open = PIPE_ACCESS_DUPLEX;
    if first {
        open |= FILE_FLAG_FIRST_PIPE_INSTANCE;
    }
    let h = unsafe {
        CreateNamedPipeW(
            &HSTRING::from(name),
            open,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            255,
            4096,
            4096,
            0,
            Some(&sa),
        )
    };
    let err = io::Error::last_os_error();
    unsafe {
        let _ = LocalFree(Some(HLOCAL(sd.0)));
    }
    if h.is_invalid() {
        return Err(err);
    }
    Ok(unsafe { OwnedHandle::from_raw_handle(h.0) })
}

impl Server {
    pub fn create_first() -> io::Result<Server> {
        Self::create_first_named(&pipe_name())
    }

    pub fn create_first_named(name: &str) -> io::Result<Server> {
        Ok(Server { name: name.to_string(), pending: create_instance(name, true)? })
    }

    pub fn accept(&mut self) -> io::Result<File> {
        let h = HANDLE(self.pending.as_raw_handle());
        if let Err(e) = unsafe { ConnectNamedPipe(h, None) } {
            if unsafe { GetLastError() } != ERROR_PIPE_CONNECTED {
                return Err(e.into());
            }
        }
        let next = create_instance(&self.name, false)?;
        Ok(File::from(std::mem::replace(&mut self.pending, next)))
    }
}

pub fn connect() -> io::Result<(File, u32)> {
    connect_to(&pipe_name())
}

/// Opens the pipe once, waiting up to 500 ms if all instances are busy.
pub fn connect_to(name: &str) -> io::Result<(File, u32)> {
    let open = || OpenOptions::new().read(true).write(true).open(name);
    let f = match open() {
        Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
            unsafe { WaitNamedPipeW(&HSTRING::from(name), 500) }.ok()?;
            open()?
        }
        r => r?,
    };
    let mut pid = 0u32;
    unsafe { GetNamedPipeServerProcessId(HANDLE(f.as_raw_handle()), &mut pid)? };
    Ok((f, pid))
}

pub fn is_not_found(e: &io::Error) -> bool {
    e.raw_os_error() == Some(ERROR_FILE_NOT_FOUND)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};

    #[test]
    fn roundtrip_line_and_server_pid() {
        let name = format!(r"\\.\pipe\ai-test-roundtrip-{}", std::process::id());
        let mut server = Server::create_first_named(&name).unwrap();
        let t = std::thread::spawn(move || {
            let f = server.accept().unwrap();
            let mut line = String::new();
            BufReader::new(&f).read_line(&mut line).unwrap();
            (&f).write_all(b"pong\n").unwrap();
            line
        });
        let (f, pid) = connect_to(&name).unwrap();
        assert_eq!(pid, std::process::id());
        (&f).write_all(b"ping\n").unwrap();
        let mut reply = String::new();
        BufReader::new(&f).read_line(&mut reply).unwrap();
        assert_eq!(reply, "pong\n");
        assert_eq!(t.join().unwrap(), "ping\n");
    }
}
