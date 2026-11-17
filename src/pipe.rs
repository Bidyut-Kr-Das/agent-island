use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use windows::Win32::Foundation::{ERROR_PIPE_CONNECTED, GetLastError, HANDLE, HLOCAL, LocalFree, WIN32_ERROR};
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
const ERROR_NO_DATA: u32 = 232;

pub fn pipe_name() -> String {
    // Test-only override so tests never touch a real island; debug builds only.
    #[cfg(debug_assertions)]
    if let Ok(n) = std::env::var("AGENT_ISLANDS_PIPE") {
        return n;
    }
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
        // ERROR_NO_DATA: the client connected, wrote and closed before this call. Its bytes stay readable.
        if let Err(e) = unsafe { ConnectNamedPipe(h, None) }
            && !matches!(unsafe { GetLastError() }, ERROR_PIPE_CONNECTED | WIN32_ERROR(ERROR_NO_DATA))
        {
            // Broken instance (e.g. client closed early): replace it so the next accept works.
            self.pending = create_instance(&self.name, false)?;
            return Err(e.into());
        }
        let next = create_instance(&self.name, false)?;
        Ok(File::from(std::mem::replace(&mut self.pending, next)))
    }
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

    #[test]
    fn client_that_wrote_and_closed_before_accept_is_still_read() {
        let name = format!(r"\\.\pipe\ai-test-closed-{}", std::process::id());
        let mut server = Server::create_first_named(&name).unwrap();
        let (mut f, _) = connect_to(&name).unwrap();
        f.write_all(b"early\n").unwrap();
        drop(f);
        let f = server.accept().unwrap();
        let mut line = String::new();
        BufReader::new(&f).read_line(&mut line).unwrap();
        assert_eq!(line, "early\n");
    }

    #[test]
    fn dropped_client_does_not_wedge_accept() {
        let name = format!(r"\\.\pipe\ai-test-wedge-{}", std::process::id());
        let mut server = Server::create_first_named(&name).unwrap();
        drop(connect_to(&name).unwrap());
        let _ = server.accept();
        let t = std::thread::spawn(move || {
            let f = server.accept().unwrap();
            let mut line = String::new();
            BufReader::new(&f).read_line(&mut line).unwrap();
            line
        });
        let (f, _) = connect_to(&name).unwrap();
        (&f).write_all(b"hi\n").unwrap();
        assert_eq!(t.join().unwrap(), "hi\n");
    }
}
