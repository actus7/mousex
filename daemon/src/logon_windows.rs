//! Primitivas Windows do serviço e do canal local. Nenhuma operação de clipboard ou arquivo remoto.
use crate::logon_protocol::{Input, agent_command};
use mousex_native::{Event, Injector};
use std::{
    ffi::{OsStr, c_void},
    io::{self, Write},
    mem::{self, size_of},
    os::windows::{ffi::OsStrExt, fs::OpenOptionsExt, io::AsRawHandle},
    path::Path,
    ptr,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicPtr, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    Security::{Authorization::*, *},
    Storage::FileSystem::{SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT},
    System::{
        JobObjects::*,
        Pipes::GetNamedPipeServerProcessId,
        RemoteDesktop::{ProcessIdToSessionId, WTSGetActiveConsoleSessionId},
        Services::*,
        StationsAndDesktops::*,
        Threading::*,
    },
};

pub const SERVICE: &str = "MouseX Logon";
const PRIVATE_ACL: &str = "D:P(A;;GA;;;SY)(A;;GA;;;BA)";
static STOP: AtomicBool = AtomicBool::new(false);
static STATUS: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());

fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(Some(0)).collect()
}

pub struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: handle próprio, fechado uma vez.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

struct Descriptor(PSECURITY_DESCRIPTOR);
impl Descriptor {
    fn new() -> io::Result<Self> {
        let mut descriptor = ptr::null_mut();
        // SAFETY: SDDL constante terminada em NUL; descritor liberado por Drop.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide(PRIVATE_ACL).as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(descriptor))
    }
    fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.0,
            bInheritHandle: 0,
        }
    }
}
impl Drop for Descriptor {
    fn drop(&mut self) {
        // SAFETY: alocado pela conversão SDDL.
        unsafe {
            LocalFree(self.0);
        }
    }
}

fn session_id() -> io::Result<u32> {
    let mut session = 0;
    // SAFETY: buffer local e PID próprio.
    if unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(session)
}

fn marker() -> Vec<u16> {
    wide(r"Global\MouseXAgent")
}

pub fn announce_agent() -> io::Result<Handle> {
    let descriptor = Descriptor::new()?;
    // SAFETY: atributos válidos durante a chamada e nome terminado em NUL.
    let handle = unsafe { CreateMutexW(&descriptor.attributes(), 0, marker().as_ptr()) };
    if handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    Ok(Handle(handle))
}

pub fn agent_present() -> Result<bool, ()> {
    // SAFETY: nome terminado em NUL; só consulta a existência, sem adquirir o mutex.
    let handle = unsafe { OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, 0, marker().as_ptr()) };
    if !handle.is_null() {
        drop(Handle(handle));
        return Ok(true);
    }
    match io::Error::last_os_error().raw_os_error() {
        Some(code) if code == ERROR_FILE_NOT_FOUND as i32 => Ok(false),
        _ => Err(()),
    }
}

pub fn pipe_name() -> io::Result<String> {
    Ok(format!(r"\\.\pipe\MouseXLogon-{}", session_id()?))
}

fn system_process(process: HANDLE) -> bool {
    let mut token = ptr::null_mut();
    // SAFETY: handles válidos; buffer alinhado para TOKEN_USER.
    unsafe {
        if OpenProcessToken(process, TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let token = Handle(token);
        let mut buffer = [0usize; 128];
        let mut length = 0;
        if GetTokenInformation(token.0, TokenUser, buffer.as_mut_ptr() as _, size_of_val(&buffer) as u32, &mut length)
            == 0
        {
            return false;
        }
        IsWellKnownSid((*(buffer.as_ptr() as *const TOKEN_USER)).User.Sid, WinLocalSystemSid) != 0
    }
}

pub fn require_system() -> io::Result<()> {
    // SAFETY: pseudo-handle próprio não precisa fechar.
    if unsafe { system_process(GetCurrentProcess()) } {
        Ok(())
    } else {
        Err(io::Error::new(io::ErrorKind::PermissionDenied, "logon mode requires LocalSystem"))
    }
}

/// Evita entregar senhas a um pipe falso criado antes do auxiliar por um usuário comum.
fn trusted_pipe(file: &std::fs::File) -> bool {
    let mut pid = 0;
    // SAFETY: handle do arquivo é um pipe aberto; processo consultado só para conferir o SID.
    unsafe {
        if GetNamedPipeServerProcessId(file.as_raw_handle() as _, &mut pid) == 0 {
            return false;
        }
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return false;
        }
        system_process(Handle(process).0)
    }
}

/// Escritor separado: pipe parado nunca bloqueia o loop da rede do agente normal.
pub struct Forwarder {
    tx: mpsc::SyncSender<Input>,
    reset: Arc<AtomicBool>,
}
impl Forwarder {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::sync_channel::<Input>(256);
        let reset = Arc::new(AtomicBool::new(false));
        let worker_reset = reset.clone();
        std::thread::spawn(move || {
            let mut file = None;
            while let Ok(input) = rx.recv() {
                if worker_reset.swap(false, Ordering::SeqCst) {
                    file = None;
                    // Descarta a sequência incompleta; fechar o pipe libera as teclas no auxiliar.
                    while rx.try_recv().is_ok() {}
                    continue;
                }
                if file.is_none() {
                    file = pipe_name()
                        .ok()
                        .and_then(|name| {
                            std::fs::OpenOptions::new()
                                .write(true)
                                // Nunca permite que o servidor do pipe personifique o agente elevado.
                                .custom_flags(SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION)
                                .open(name)
                                .ok()
                        })
                        .filter(trusted_pipe);
                }
                if let Some(pipe) = &mut file
                    && pipe.write_all(&input.encode()).is_err()
                {
                    file = None;
                }
            }
        });
        Self { tx, reset }
    }
    pub fn send(&self, input: Input) {
        if self.tx.try_send(input).is_err() {
            self.reset.store(true, Ordering::SeqCst);
            crate::log("logon input queue unavailable");
        }
    }
}

fn desktop_name(handle: HDESK) -> io::Result<Vec<u16>> {
    let mut buffer = [0u16; 256];
    let mut needed = 0;
    // SAFETY: handle de desktop válido e buffer UTF-16 do tamanho informado em bytes.
    if unsafe {
        GetUserObjectInformationW(handle, UOI_NAME, buffer.as_mut_ptr() as _, size_of_val(&buffer) as u32, &mut needed)
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(buffer.into_iter().take_while(|c| *c != 0).collect())
}

pub fn secure_desktop() -> bool {
    // Falha de acesso também indica desktop protegido; o auxiliar é quem pode abri-lo.
    unsafe {
        let desktop = OpenInputDesktop(0, 0, DESKTOP_READOBJECTS);
        if desktop.is_null() {
            return true;
        }
        let secure = desktop_name(desktop).map_or(true, |name| name != wide("Default")[..7]);
        CloseDesktop(desktop);
        secure
    }
}

/// Só esta thread injeta. Ela nunca cria janelas/hooks e nunca migra entre threads do Tokio.
#[derive(Clone)]
pub struct InputWorker(mpsc::SyncSender<Input>);
impl InputWorker {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::sync_channel::<Input>(256);
        std::thread::spawn(move || {
            // SAFETY: handle emprestado do desktop inicial, mantido pelo Windows durante a thread.
            let original = unsafe { GetThreadDesktop(GetCurrentThreadId()) };
            let mut desktop: HDESK = ptr::null_mut();
            let mut name = Vec::new();
            let mut injector = Injector::default();
            for input in rx {
                // SAFETY: thread sem janelas/hooks; só fecha handles próprios depois de trocar.
                unsafe {
                    let current =
                        OpenInputDesktop(0, 0, DESKTOP_READOBJECTS | DESKTOP_WRITEOBJECTS | DESKTOP_SWITCHDESKTOP);
                    if current.is_null() {
                        continue;
                    }
                    let Ok(current_name) = desktop_name(current) else {
                        CloseDesktop(current);
                        continue;
                    };
                    if name != current_name {
                        if SetThreadDesktop(current) == 0 {
                            CloseDesktop(current);
                            continue;
                        }
                        if !desktop.is_null() {
                            CloseDesktop(desktop);
                        }
                        desktop = current;
                        name = current_name;
                        let _ = injector.release_all();
                    } else {
                        CloseDesktop(current);
                    }
                }
                let _ = inject(&mut injector, input);
            }
            drop(injector);
            // SAFETY: restaura o desktop emprestado antes de fechar o último handle próprio.
            if !desktop.is_null() {
                unsafe {
                    if SetThreadDesktop(original) != 0 {
                        CloseDesktop(desktop);
                    }
                }
            }
        });
        Self(tx)
    }
    pub fn send(&self, input: Input) -> io::Result<()> {
        self.0.send(input).map_err(io::Error::other)
    }
}

pub fn inject(injector: &mut Injector, input: Input) -> io::Result<()> {
    match input {
        Input::Key { hid, down } => {
            if let Some((scancode, extended)) = mousex_core::keymap::hid_to_win_scancode(hid) {
                injector.send(Event::Key { scancode, extended, down })?;
            }
        }
        Input::Move { x, y } => {
            injector.send(Event::Move { x, y })?;
        }
        Input::Button { button, down } => {
            if let Some(button) = crate::platform::button_from_u8(button) {
                injector.send(Event::Button { button, down })?;
            }
        }
        Input::Wheel { dx, dy } => {
            for (delta, horizontal) in [(dy, false), (dx, true)] {
                if delta != 0 {
                    injector.send(Event::Wheel { delta: delta.clamp(-32768, 32767) as i16, horizontal })?;
                }
            }
        }
        Input::Release => {
            injector.release_all()?;
        }
    }
    // ponytail: Ctrl+Alt+Del não é encaminhado; o Windows principal consome a combinação. Sem SendSAS.
    Ok(())
}

pub fn create_pipe() -> io::Result<tokio::net::windows::named_pipe::NamedPipeServer> {
    create_pipe_named(&pipe_name()?, true)
}

pub fn next_pipe() -> io::Result<tokio::net::windows::named_pipe::NamedPipeServer> {
    create_pipe_named(&pipe_name()?, false)
}

fn create_pipe_named(name: &str, first: bool) -> io::Result<tokio::net::windows::named_pipe::NamedPipeServer> {
    let descriptor = Descriptor::new()?;
    let mut attributes = descriptor.attributes();
    // SAFETY: descritor vivo durante CreateNamedPipeW; Windows copia a DACL. Recusa conexões remotas.
    unsafe {
        tokio::net::windows::named_pipe::ServerOptions::new()
            .first_pipe_instance(first)
            .access_outbound(false)
            .reject_remote_clients(true)
            .max_instances(2)
            .create_with_security_attributes_raw(name, &mut attributes as *mut _ as _)
    }
}

fn report(state: u32, error: u32) {
    let status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: if state == SERVICE_RUNNING {
            SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN | SERVICE_ACCEPT_SESSIONCHANGE
        } else {
            0
        },
        dwWin32ExitCode: error,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: 0,
        dwWaitHint: if state == SERVICE_STOP_PENDING { 10000 } else { 0 },
    };
    // SAFETY: handle registrado pelo SCM; estrutura local do tamanho correto.
    unsafe {
        SetServiceStatus(STATUS.load(Ordering::SeqCst), &status);
    }
}

unsafe extern "system" fn control(code: u32, _: u32, _: *mut c_void, _: *mut c_void) -> u32 {
    match code {
        SERVICE_CONTROL_STOP | SERVICE_CONTROL_SHUTDOWN => {
            STOP.store(true, Ordering::SeqCst);
            report(SERVICE_STOP_PENDING, 0);
        }
        // O loop consulta a sessão do console a cada 250 ms, inclusive após logon/logoff.
        SERVICE_CONTROL_SESSIONCHANGE => {}
        SERVICE_CONTROL_INTERROGATE => {}
        _ => return ERROR_CALL_NOT_IMPLEMENTED,
    }
    NO_ERROR
}

unsafe extern "system" fn service_main(_: u32, _: *mut *mut u16) {
    let handle = unsafe { RegisterServiceCtrlHandlerExW(wide(SERVICE).as_ptr(), Some(control), ptr::null_mut()) };
    if handle.is_null() {
        return;
    }
    STATUS.store(handle, Ordering::SeqCst);
    report(SERVICE_RUNNING, 0);
    let result = supervise();
    if let Err(error) = &result {
        crate::log(&format!("logon service: {error}"));
    }
    report(SERVICE_STOPPED, if result.is_ok() { 0 } else { ERROR_SERVICE_SPECIFIC_ERROR });
}

pub fn service() -> io::Result<()> {
    require_system()?;
    let mut name = wide(SERVICE);
    let table = [
        SERVICE_TABLE_ENTRYW { lpServiceName: name.as_mut_ptr(), lpServiceProc: Some(service_main) },
        SERVICE_TABLE_ENTRYW { lpServiceName: ptr::null_mut(), lpServiceProc: None },
    ];
    // SAFETY: table e nome vivem até o dispatcher retornar.
    if unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

struct Helper {
    process: Handle,
    _job: Handle,
}
impl Drop for Helper {
    fn drop(&mut self) {
        // SAFETY: processo próprio; job kill-on-close também cobre encerramento abrupto do serviço.
        unsafe {
            TerminateProcess(self.process.0, 0);
            WaitForSingleObject(self.process.0, 5000);
        }
    }
}

fn spawn_helper(exe: &Path, session: u32) -> io::Result<Helper> {
    let command = agent_command(exe.to_str().ok_or_else(|| io::Error::other("non-Unicode agent path"))?, "--logon")
        .ok_or_else(|| io::Error::other("invalid agent path"))?;
    let mut command = wide(command);
    let mut desktop = wide(r"winsta0\winlogon");
    // SAFETY: buffers válidos, handles próprios; filho suspenso até entrar no job.
    unsafe {
        let mut token = ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_ALL_ACCESS, &mut token) == 0 {
            return Err(io::Error::last_os_error());
        }
        let token = Handle(token);
        let mut luid = mem::zeroed();
        if LookupPrivilegeValueW(ptr::null(), wide("SeTcbPrivilege").as_ptr(), &mut luid) == 0 {
            return Err(io::Error::last_os_error());
        }
        let privileges = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: SE_PRIVILEGE_ENABLED }],
        };
        if AdjustTokenPrivileges(token.0, 0, &privileges, 0, ptr::null_mut(), ptr::null_mut()) == 0
            || GetLastError() == ERROR_NOT_ALL_ASSIGNED
        {
            return Err(io::Error::last_os_error());
        }
        let mut primary = ptr::null_mut();
        if DuplicateTokenEx(token.0, TOKEN_ALL_ACCESS, ptr::null(), SecurityImpersonation, TokenPrimary, &mut primary)
            == 0
        {
            return Err(io::Error::last_os_error());
        }
        let primary = Handle(primary);
        if SetTokenInformation(primary.0, TokenSessionId, &session as *const _ as _, size_of::<u32>() as u32) == 0 {
            return Err(io::Error::last_os_error());
        }
        let job = CreateJobObjectW(ptr::null(), ptr::null());
        if job.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = Handle(job);
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = mem::zeroed();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if SetInformationJobObject(
            job.0,
            JobObjectExtendedLimitInformation,
            &limits as *const _ as _,
            size_of_val(&limits) as u32,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let mut startup: STARTUPINFOW = mem::zeroed();
        startup.cb = size_of_val(&startup) as u32;
        startup.lpDesktop = desktop.as_mut_ptr();
        let mut info: PROCESS_INFORMATION = mem::zeroed();
        if CreateProcessAsUserW(
            primary.0,
            wide(exe).as_ptr(),
            command.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            0,
            CREATE_SUSPENDED | CREATE_NO_WINDOW,
            ptr::null(),
            wide(exe.parent().ok_or_else(|| io::Error::other("agent folder unavailable"))?).as_ptr(),
            &startup,
            &mut info,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let child = Helper { process: Handle(info.hProcess), _job: job };
        let thread = Handle(info.hThread);
        if AssignProcessToJobObject(child._job.0, child.process.0) == 0 || ResumeThread(thread.0) == u32::MAX {
            return Err(io::Error::last_os_error());
        }
        Ok(child)
    }
}

fn supervise() -> io::Result<()> {
    let exe = std::env::current_exe()?;
    let mut child: Option<Helper> = None;
    let mut session = u32::MAX;
    let mut retry = Instant::now();
    let mut started = Instant::now();
    let mut delay = Duration::from_secs(1);
    while !STOP.load(Ordering::SeqCst) {
        // SAFETY: consultas sem buffers.
        let active = unsafe { WTSGetActiveConsoleSessionId() };
        if active != session {
            child = None;
            session = active;
            delay = Duration::from_secs(1);
            retry = Instant::now();
        }
        if child.as_ref().is_some_and(|c| unsafe { WaitForSingleObject(c.process.0, 0) } != WAIT_TIMEOUT) {
            child = None;
            delay = if started.elapsed() >= Duration::from_secs(60) {
                Duration::from_secs(1)
            } else {
                (delay * 2).min(Duration::from_secs(30))
            };
            retry = Instant::now() + delay;
        }
        if child.is_none() && session != u32::MAX && Instant::now() >= retry {
            match spawn_helper(&exe, session) {
                Ok(helper) => {
                    child = Some(helper);
                    started = Instant::now();
                }
                Err(error) => {
                    crate::log(&format!("logon helper could not start: {error}"));
                    retry = Instant::now() + delay;
                    delay = (delay * 2).min(Duration::from_secs(30));
                }
            }
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    drop(child);
    Ok(())
}

struct ServiceHandle(SC_HANDLE);
impl Drop for ServiceHandle {
    fn drop(&mut self) {
        // SAFETY: handle SCM próprio.
        unsafe {
            CloseServiceHandle(self.0);
        }
    }
}

fn manager(create: bool) -> io::Result<ServiceHandle> {
    // SAFETY: computador e banco locais; sem ponteiros de saída.
    let handle = unsafe {
        OpenSCManagerW(
            ptr::null(),
            ptr::null(),
            SC_MANAGER_CONNECT | if create { SC_MANAGER_CREATE_SERVICE } else { 0 },
        )
    };
    if handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    Ok(ServiceHandle(handle))
}

fn existing(manager: &ServiceHandle) -> io::Result<Option<ServiceHandle>> {
    // SAFETY: nome constante terminado em NUL; handle válido do SCM.
    let handle = unsafe { OpenServiceW(manager.0, wide(SERVICE).as_ptr(), SERVICE_ALL_ACCESS) };
    if !handle.is_null() {
        return Ok(Some(ServiceHandle(handle)));
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(ERROR_SERVICE_DOES_NOT_EXIST as i32) { Ok(None) } else { Err(error) }
}

fn service_state(service: &ServiceHandle) -> io::Result<u32> {
    let mut status: SERVICE_STATUS = unsafe { mem::zeroed() };
    // SAFETY: estrutura de saída do tamanho correto.
    if unsafe { QueryServiceStatus(service.0, &mut status) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(status.dwCurrentState)
}

fn wait_service(service: &ServiceHandle, state: u32) -> io::Result<()> {
    for _ in 0..150 {
        if service_state(service)? == state {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(io::Error::new(io::ErrorKind::TimedOut, "logon service did not change state within 15 s"))
}

pub fn stop_service() -> io::Result<()> {
    let manager = manager(false)?;
    let Some(service) = existing(&manager)? else {
        return Ok(());
    };
    let state = service_state(&service)?;
    if state == SERVICE_STOPPED {
        return Ok(());
    }
    if state != SERVICE_STOP_PENDING {
        let mut status = unsafe { mem::zeroed() };
        // SAFETY: handle válido e buffer de status local.
        if unsafe { ControlService(service.0, SERVICE_CONTROL_STOP, &mut status) } == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_SERVICE_NOT_ACTIVE as i32) {
                return Err(error);
            }
        }
    }
    wait_service(&service, SERVICE_STOPPED)
}

pub fn remove_service() -> io::Result<()> {
    stop_service()?;
    let manager = manager(false)?;
    if let Some(service) = existing(&manager)? {
        // SAFETY: handle aberto com direito DELETE.
        if unsafe { DeleteService(service.0) } == 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Idempotente: também migra clientes antigos depois da atualização automática.
pub fn install_service(exe: &Path) -> io::Result<()> {
    let manager = manager(true)?;
    let command = agent_command(exe.to_str().ok_or_else(|| io::Error::other("non-Unicode agent path"))?, "--service")
        .ok_or_else(|| io::Error::other("invalid service path"))?;
    let command = wide(command);
    let name = wide(SERVICE);
    let account = wide("LocalSystem");
    let service = match existing(&manager)? {
        Some(service) => {
            // SAFETY: configuração local; caminho sempre citado, conta sempre LocalSystem.
            if unsafe {
                ChangeServiceConfigW(
                    service.0,
                    SERVICE_WIN32_OWN_PROCESS,
                    SERVICE_AUTO_START,
                    SERVICE_ERROR_NORMAL,
                    command.as_ptr(),
                    ptr::null(),
                    ptr::null_mut(),
                    ptr::null(),
                    account.as_ptr(),
                    ptr::null(),
                    name.as_ptr(),
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            service
        }
        None => {
            // SAFETY: strings válidas; serviço novo fechado por RAII.
            let handle = unsafe {
                CreateServiceW(
                    manager.0,
                    name.as_ptr(),
                    name.as_ptr(),
                    SERVICE_ALL_ACCESS,
                    SERVICE_WIN32_OWN_PROCESS,
                    SERVICE_AUTO_START,
                    SERVICE_ERROR_NORMAL,
                    command.as_ptr(),
                    ptr::null(),
                    ptr::null_mut(),
                    ptr::null(),
                    account.as_ptr(),
                    ptr::null(),
                )
            };
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            ServiceHandle(handle)
        }
    };
    // Reinicia o próprio serviço se ele cair; o auxiliar já tem seu backoff separado.
    let mut action = SC_ACTION { Type: SC_ACTION_RESTART, Delay: 5000 };
    let failures = SERVICE_FAILURE_ACTIONSW {
        dwResetPeriod: 60,
        lpRebootMsg: ptr::null_mut(),
        lpCommand: ptr::null_mut(),
        cActions: 1,
        lpsaActions: &mut action,
    };
    // SAFETY: estruturas e strings permanecem vivas durante as chamadas.
    unsafe {
        if ChangeServiceConfig2W(service.0, SERVICE_CONFIG_FAILURE_ACTIONS, &failures as *const _ as _) == 0 {
            return Err(io::Error::last_os_error());
        }
        if StartServiceW(service.0, 0, ptr::null()) == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_SERVICE_ALREADY_RUNNING as i32) {
                return Err(error);
            }
        }
    }
    wait_service(&service, SERVICE_RUNNING)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn pipe_de_entrada_identifica_servidor_com_handle_so_de_escrita() {
        use tokio::{io::AsyncReadExt, net::windows::named_pipe::ServerOptions};
        let name = format!(r"\\.\pipe\MouseXLogon-writer-test-{}", rand::random::<u64>());
        // ACL do usuário do teste: testa o modo do transporte sem iniciar um processo SYSTEM.
        let mut server = ServerOptions::new()
            .first_pipe_instance(true)
            .access_outbound(false)
            .reject_remote_clients(true)
            .create(&name)
            .expect("pipe temporário");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION)
            .open(&name)
            .expect("escritor");
        let mut pid = 0;
        // SAFETY: handle de pipe válido, PID como saída.
        assert_ne!(unsafe { GetNamedPipeServerProcessId(file.as_raw_handle() as _, &mut pid) }, 0);
        assert_eq!(pid, std::process::id());
        // O processo comum do teste não pode passar pela verificação de servidor SYSTEM.
        if require_system().is_err() {
            assert!(!trusted_pipe(&file));
        }
        server.connect().await.expect("conexão");
        let packet = Input::Key { hid: 4, down: true }.encode();
        file.write_all(&packet).expect("escrita");
        let mut received = [0u8; crate::logon_protocol::PACKET_SIZE];
        tokio::time::timeout(Duration::from_secs(2), server.read_exact(&mut received))
            .await
            .expect("prazo")
            .expect("leitura");
        assert_eq!(received, packet);
    }

    #[tokio::test]
    async fn pipe_tem_apenas_system_e_administradores_e_nao_aceita_instancia_falsa() {
        let name = format!(r"\\.\pipe\MouseXLogon-test-{}-{}", std::process::id(), rand::random::<u64>());
        let pipe = create_pipe_named(&name, true).expect("pipe temporário");
        assert!(create_pipe_named(&name, true).is_err(), "nome já ocupado deve falhar fechado");
        // SAFETY: pipe válido, saídas locais; descritor devolvido é liberado por Descriptor.
        unsafe {
            let mut descriptor = ptr::null_mut();
            let mut acl = ptr::null_mut();
            assert_eq!(
                GetSecurityInfo(
                    pipe.as_raw_handle() as _,
                    SE_KERNEL_OBJECT,
                    DACL_SECURITY_INFORMATION,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    &mut acl,
                    ptr::null_mut(),
                    &mut descriptor
                ),
                ERROR_SUCCESS
            );
            let descriptor = Descriptor(descriptor);
            assert!(!acl.is_null());
            assert_eq!((*acl).AceCount, 2);
            let mut control = 0;
            let mut revision = 0;
            assert_ne!(GetSecurityDescriptorControl(descriptor.0, &mut control, &mut revision), 0);
            assert_ne!(control & SE_DACL_PROTECTED, 0, "sem herança");
            let mut system = false;
            let mut admins = false;
            for index in 0..2 {
                let mut ace = ptr::null_mut();
                assert_ne!(GetAce(acl, index, &mut ace), 0);
                let ace = &*(ace as *const ACCESS_ALLOWED_ACE);
                assert_eq!(ace.Header.AceType, 0, "ACCESS_ALLOWED_ACE_TYPE");
                let sid = &ace.SidStart as *const _ as _;
                system |= IsWellKnownSid(sid, WinLocalSystemSid) != 0;
                admins |= IsWellKnownSid(sid, WinBuiltinAdministratorsSid) != 0;
            }
            assert!(system && admins);
        }
    }
}
