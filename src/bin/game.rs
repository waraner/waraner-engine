use std::ffi::c_void;

type WaranerCreateContext = unsafe extern "C" fn(config: *const c_void) -> *mut c_void;
type WaranerRun = unsafe extern "C" fn(*mut c_void);
type WaranerDestroyContext = unsafe extern "C" fn(*mut c_void);

fn main() {
    let lib_name = if cfg!(target_os = "windows") {
        "bin\\waraner_client.dll"
    } else if cfg!(target_os = "macos") {
        "bin/libwaraner_client.dylib"
    } else {
        "bin/libwaraner_client.so"
    };

    let lib = unsafe {
        libloading::Library::new(lib_name)
            .unwrap_or_else(|e| panic!("Failed to load {}: {}", lib_name, e))
    };

    let create: libloading::Symbol<WaranerCreateContext> = unsafe {
        lib.get(b"waraner_create_context")
            .expect("symbol waraner_create_context not found")
    };
    let run: libloading::Symbol<WaranerRun> = unsafe {
        lib.get(b"waraner_run")
            .expect("symbol waraner_run not found")
    };
    let destroy: libloading::Symbol<WaranerDestroyContext> = unsafe {
        lib.get(b"waraner_destroy_context")
            .expect("symbol waraner_destroy_context not found")
    };

    // Pass null config to use defaults + env/args
    let ctx = unsafe { create(std::ptr::null()) };
    unsafe { run(ctx) };
    unsafe { destroy(ctx) };
}
