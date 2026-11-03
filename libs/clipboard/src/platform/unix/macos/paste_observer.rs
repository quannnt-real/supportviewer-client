use super::pasteboard_context::PasteObserverInfo;
use fsevent::{self, StreamFlags};
use hbb_common::{bail, log, ResultType};
use std::{
    sync::{
        mpsc::{channel, Receiver, RecvTimeoutError, Sender},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

enum FseventControl {
    Start,
    Stop,
    Exit,
}

struct FseventThreadInfo {
    tx: Sender<FseventControl>,
    handle: thread::JoinHandle<()>,
}

pub struct PasteObserver {
    exit: Arc<Mutex<bool>>,
    observer_info: Arc<Mutex<Option<PasteObserverInfo>>>,
    tx_handle_fsevent_thread: Option<FseventThreadInfo>,
    handle_observer_thread: Option<thread::JoinHandle<()>>,
}

impl Drop for PasteObserver {
    fn drop(&mut self) {
        *self.exit.lock().unwrap() = true;
        if let Some(handle_observer_thread) = self.handle_observer_thread.take() {
            handle_observer_thread.join().ok();
        }
        if let Some(tx_handle_fsevent_thread) = self.tx_handle_fsevent_thread.take() {
            tx_handle_fsevent_thread.tx.send(FseventControl::Exit).ok();
            tx_handle_fsevent_thread.handle.join().ok();
        }
    }
}

impl PasteObserver {
    const OBSERVE_TIMEOUT: Duration = Duration::from_secs(30);

    pub fn new() -> Self {
        Self {
            exit: Arc::new(Mutex::new(false)),
            observer_info: Default::default(),
            tx_handle_fsevent_thread: None,
            handle_observer_thread: None,
        }
    }

    pub fn init(&mut self, cb_pasted: fn(&PasteObserverInfo) -> ()) -> ResultType<()> {
        let Some(home_dir) = dirs::home_dir() else {
            bail!("No home dir is set, do not observe.");
        };

        let (tx_observer, rx_observer) = channel::<fsevent::Event>();
        let handle_observer = Self::init_thread_observer(
            self.exit.clone(),
            self.observer_info.clone(),
            rx_observer,
            cb_pasted,
        );
        self.handle_observer_thread = Some(handle_observer);
        let (tx_control, rx_control) = channel::<FseventControl>();
        let handle_fsevent = Self::init_thread_fsevent(
            home_dir.to_string_lossy().to_string(),
            tx_observer,
            rx_control,
        );
        self.tx_handle_fsevent_thread = Some(FseventThreadInfo {
            tx: tx_control,
            handle: handle_fsevent,
        });
        Ok(())
    }

    #[inline]
    fn get_file_from_path(path: &String) -> String {
        let last_slash = path.rfind('/').or_else(|| path.rfind('\\'));
        match last_slash {
            Some(index) => path[index + 1..].to_string(),
            None => path.clone(),
        }
    }

    fn init_thread_observer(
        exit: Arc<Mutex<bool>>,
        observer_info: Arc<Mutex<Option<PasteObserverInfo>>>,
        rx_observer: Receiver<fsevent::Event>,
        cb_pasted: fn(&PasteObserverInfo) -> (),
    ) -> thread::JoinHandle<()> {
        thread::spawn(move || loop {
            match rx_observer.recv_timeout(Duration::from_millis(300)) {
                Ok(event) => {
                    if (event.flag
                        & (StreamFlags::ITEM_CREATED
                            | StreamFlags::ITEM_RENAMED
                            | StreamFlags::ITEM_MODIFIED))
                        != StreamFlags::NONE
                        && (event.flag & StreamFlags::ITEM_REMOVED) == StreamFlags::NONE
                        && (event.flag & StreamFlags::IS_FILE) != StreamFlags::NONE
                    {
                        let matched_info = {
                            let mut guard = observer_info.lock().unwrap();
                            let is_match = guard.as_ref().map_or(false, |x| {
                                !x.source_path.is_empty()
                                    && event.path != x.source_path
                                    && Self::get_file_from_path(&x.source_path)
                                        == Self::get_file_from_path(&event.path)
                            });
                            if is_match {
                                guard.take()
                            } else {
                                None
                            }
                        };
                        if let Some(mut info) = matched_info {
                            info.target_path = event.path.clone();
                            cb_pasted(&info);
                        }
                    }
                }
                Err(_) => {
                    if *(exit.lock().unwrap()) {
                        break;
                    }
                }
            }
        })
    }

    fn collect_observe_dirs(home_dir: &str) -> Vec<String> {
        let mut dirs = vec![home_dir.to_string()];
        let volumes = std::path::Path::new("/Volumes");
        if volumes.is_dir() {
            if let Ok(entries) = std::fs::read_dir(volumes) {
                for entry in entries.flatten() {
                    if let Ok(ft) = entry.file_type() {
                        // Skip symlinks (e.g. "/Volumes/Macintosh HD -> /") so we only watch
                        // secondary/external volumes rather than the entire root filesystem.
                        if ft.is_dir() && !ft.is_symlink() {
                            let vol_path = entry.path().to_string_lossy().to_string();
                            if !dirs.contains(&vol_path) {
                                dirs.push(vol_path);
                            }
                        }
                    }
                }
            }
            let volumes_root = "/Volumes".to_string();
            if !dirs.contains(&volumes_root) {
                dirs.push(volumes_root);
            }
        }
        dirs
    }

    fn new_fsevent(home_dir: String, tx_observer: Sender<fsevent::Event>) -> fsevent::FsEvent {
        let dirs = Self::collect_observe_dirs(&home_dir);
        log::debug!("fsevent observe dirs: {:?}", &dirs);
        let mut evt = fsevent::FsEvent::new(dirs);
        evt.observe_async(tx_observer).ok();
        evt
    }

    fn init_thread_fsevent(
        home_dir: String,
        tx_observer: Sender<fsevent::Event>,
        rx_control: Receiver<FseventControl>,
    ) -> thread::JoinHandle<()> {
        log::debug!("fsevent home dir: {}", &home_dir);
        thread::spawn(move || {
            let mut fsevent = None;
            loop {
                match rx_control.recv_timeout(Self::OBSERVE_TIMEOUT) {
                    Ok(FseventControl::Start) => {
                        if fsevent.is_none() {
                            fsevent =
                                Some(Self::new_fsevent(home_dir.clone(), tx_observer.clone()));
                        }
                    }
                    Ok(FseventControl::Stop) | Err(RecvTimeoutError::Timeout) => {
                        let _ = fsevent.as_mut().map(|e| e.shutdown_observe());
                        fsevent = None;
                    }
                    Ok(FseventControl::Exit) | Err(RecvTimeoutError::Disconnected) => {
                        break;
                    }
                }
            }
            log::info!("fsevent thread exit");
            let _ = fsevent.as_mut().map(|e| e.shutdown_observe());
        })
    }

    pub fn prewarm(&self) {
        if let Some(tx_handle_fsevent_thread) = self.tx_handle_fsevent_thread.as_ref() {
            tx_handle_fsevent_thread.tx.send(FseventControl::Start).ok();
        }
    }

    pub fn start(&mut self, observer_info: PasteObserverInfo) {
        if let Some(tx_handle_fsevent_thread) = self.tx_handle_fsevent_thread.as_ref() {
            self.observer_info.lock().unwrap().replace(observer_info);
            tx_handle_fsevent_thread.tx.send(FseventControl::Start).ok();
        }
    }

    pub fn stop(&mut self) {
        if let Some(tx_handle_fsevent_thread) = &self.tx_handle_fsevent_thread {
            self.observer_info = Default::default();
            tx_handle_fsevent_thread.tx.send(FseventControl::Stop).ok();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_observe_dirs_includes_home_and_mounted_volumes_without_symlinks() {
        let dirs = PasteObserver::collect_observe_dirs("/Users/testuser");
        assert_eq!(dirs.first().map(String::as_str), Some("/Users/testuser"));
        if std::path::Path::new("/Volumes").is_dir() {
            assert!(dirs.iter().any(|d| d == "/Volumes"));
            for d in &dirs {
                if let Some(rest) = d.strip_prefix("/Volumes/") {
                    if !rest.is_empty() {
                        let meta = std::fs::symlink_metadata(d).unwrap();
                        assert!(!meta.file_type().is_symlink(), "symlink leaked: {d}");
                    }
                }
            }
        }
    }
}
