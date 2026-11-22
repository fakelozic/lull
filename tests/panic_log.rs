//! A panic must reach the log with its place. Alone in its file, because the logger is process-wide.

#[test]
fn a_panic_is_logged_with_file_and_line() {
    let folder = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("panic-log");
    let _ = std::fs::remove_dir_all(&folder);
    lull::log::start(lull::log::Process::Sound, "test", &folder, std::path::Path::new("/nonexistent-home"));
    let thread = std::thread::Builder::new().name("audio".into()).spawn(|| panic!("pretend breakage")).unwrap();
    assert!(thread.join().is_err());
    let file = std::fs::read_dir(&folder).unwrap().next().unwrap().unwrap().path();
    let text = std::fs::read_to_string(file).unwrap();
    let line: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!((line["event"].as_str(), line["level"].as_str()), (Some("process.panic"), Some("error")));
    let detail = line["detail"].as_str().unwrap();
    assert!(detail.contains("thread=audio") && detail.contains("panic_log.rs:") && detail.contains("pretend breakage"));
}
