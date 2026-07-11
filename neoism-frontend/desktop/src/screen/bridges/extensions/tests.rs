
use super::*;

#[test]
fn remove_managed_python_kernelspec_dir_deletes_stale_kernel_tree() {
    let tmp = tempfile::tempdir().unwrap();
    let kernel_dir = tmp
        .path()
        .join("jupyter")
        .join("share")
        .join("jupyter")
        .join("kernels")
        .join("python3");
    std::fs::create_dir_all(&kernel_dir).unwrap();
    std::fs::write(kernel_dir.join("kernel.json"), "{}").unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let first = runtime.block_on(remove_managed_python_kernelspec_dir(&kernel_dir));
    assert!(first.is_ok());
    assert!(!kernel_dir.exists());

    let second = runtime.block_on(remove_managed_python_kernelspec_dir(&kernel_dir));
    assert!(second.is_ok());
}

#[test]
fn install_command_output_detail_preserves_stderr_and_stdout() {
    let detail = install_command_output_detail(b"created venv\n", b"missing lib\n");

    assert!(detail.contains("stderr:\nmissing lib"));
    assert!(detail.contains("stdout:\ncreated venv"));
}
