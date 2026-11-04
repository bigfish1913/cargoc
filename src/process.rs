//! Process-tree ownership: never kill an unrelated Cargo invocation.
use crate::{Result, fail};
use std::{
    process::{Child, Command, ExitStatus},
    thread,
    time::{Duration, Instant},
};

pub struct OwnedChild {
    child: Child,
    #[cfg(windows)]
    job: Job,
}

impl OwnedChild {
    pub fn spawn(command: &mut Command) -> Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        #[cfg(windows)]
        let job = Job::new()?;
        let child = command.spawn()?;
        #[cfg(windows)]
        let mut child = child;
        #[cfg(windows)]
        if let Err(e) = job.assign(&child) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }
        Ok(Self {
            child,
            #[cfg(windows)]
            job,
        })
    }

    pub fn status(&mut self) -> Result<Option<ExitStatus>> {
        Ok(self.child.try_wait()?)
    }

    pub fn stop(&mut self) -> Result<()> {
        #[cfg(windows)]
        self.job.terminate()?;
        #[cfg(unix)]
        {
            // SAFETY: Negative PID addresses only the process group created above.
            #[allow(unsafe_code)]
            unsafe {
                libc::kill(-(self.child.id() as i32), libc::SIGTERM);
            }
        }
        let start = Instant::now();
        while self.child.try_wait()?.is_none() {
            if start.elapsed() > Duration::from_secs(3) {
                #[cfg(unix)]
                {
                    // SAFETY: Same owned process group; no shell/process-name matching.
                    #[allow(unsafe_code)]
                    unsafe {
                        libc::kill(-(self.child.id() as i32), libc::SIGKILL);
                    }
                }
                let _ = self.child.kill();
                if start.elapsed() > Duration::from_secs(10) {
                    return fail("Cannot confirm owned Cargo stopped");
                }
            }
            thread::sleep(Duration::from_millis(25));
        }
        Ok(())
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            // Kill any orphaned build-script descendants before releasing the lease.
            // SAFETY: Group ID belongs to this invocation's group, not all cargo/rustc.
            #[allow(unsafe_code)]
            unsafe {
                libc::kill(-(self.child.id() as i32), libc::SIGKILL);
            }
        }
        if self.child.try_wait().is_ok_and(|s| s.is_none()) {
            let _ = self.stop();
        }
        // Windows Job's KILL_ON_JOB_CLOSE also handles orphaned descendants.
    }
}

#[cfg(windows)]
struct Job(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl Job {
    fn new() -> Result<Self> {
        use windows_sys::Win32::System::JobObjects::{
            CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        };
        // SAFETY: Null security/name parameters request a private unnamed job.
        #[allow(unsafe_code)]
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(std::io::Error::last_os_error().into());
        }
        let job = Self(handle);
        // SAFETY: This C POD structure permits all-zero initialization.
        #[allow(unsafe_code)]
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: A live handle and a correctly sized, initialized structure.
        #[allow(unsafe_code)]
        if unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&info).cast(),
                std::mem::size_of_val(&info) as u32,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(job)
    }

    fn assign(&self, child: &Child) -> Result<()> {
        use std::os::windows::io::AsRawHandle;
        // SAFETY: Both handles remain owned/live for the duration of this call.
        #[allow(unsafe_code)]
        if unsafe {
            windows_sys::Win32::System::JobObjects::AssignProcessToJobObject(
                self.0,
                child.as_raw_handle(),
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }

    fn terminate(&self) -> Result<()> {
        // SAFETY: This is the private job created exclusively for our subprocess.
        #[allow(unsafe_code)]
        if unsafe { windows_sys::Win32::System::JobObjects::TerminateJobObject(self.0, 2) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }
}

#[cfg(windows)]
impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: This owner closes its one live job handle exactly once.
        #[allow(unsafe_code)]
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sleeper() -> Command {
        #[cfg(windows)]
        {
            let mut command = Command::new("powershell.exe");
            command.args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Start-Sleep -Seconds 30",
            ]);
            command
        }
        #[cfg(unix)]
        {
            let mut command = Command::new("sleep");
            command.arg("30");
            command
        }
    }

    #[test]
    fn stopping_owned_tree_does_not_stop_unrelated_process() {
        let mut unrelated = sleeper().spawn().unwrap();
        let mut owned = OwnedChild::spawn(&mut sleeper()).unwrap();
        assert!(owned.status().unwrap().is_none());
        owned.stop().unwrap();
        assert!(owned.status().unwrap().is_some());
        let unrelated_alive = unrelated.try_wait().unwrap().is_none();
        let _ = unrelated.kill();
        let _ = unrelated.wait();
        assert!(unrelated_alive, "unrelated process was killed");
    }
}
