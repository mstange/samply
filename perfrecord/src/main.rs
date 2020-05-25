use cocoa::base::{id, YES};
use cocoa::foundation::{NSArray, NSData, NSDictionary, NSString, NSUInteger};
use objc::rc::autoreleasepool;
use objc::{class, msg_send, sel, sel_impl};
use uuid::Uuid;
use std::ffi::CStr;
use which::which;
use std::{thread, time};

mod process_launcher;

use process_launcher::{ProcessLauncher, MachError, mach_port_t};

#[cfg(target_os = "macos")]
#[link(name = "Symbolication", kind = "framework")]
extern "C" {
    #[no_mangle]
    #[link_name = "OBJC_CLASS_$_VMUProcessDescription"]
    static VMUProcessDescription_class: objc::runtime::Class;

    #[no_mangle]
    #[link_name = "OBJC_CLASS_$_VMUSampler"]
    static VMUSampler_class: objc::runtime::Class;
}

fn main() -> Result<(), MachError> {
    let env: Vec<_> = std::env::vars()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect();
    let env: Vec<&str> = env.iter().map(std::ops::Deref::deref).collect();

    let args: Vec<_> = std::env::args().skip(1).collect();
    let command = match args.first() {
        Some(command) => which(command).unwrap(),
        None => {
            println!("Usage: perfrecord somecommand");
            panic!()
        }
    };
    let args: Vec<&str> = args.iter().map(std::ops::Deref::deref).collect();

    let mut launcher = ProcessLauncher::new(
        &command,
        &args,
        &env,
    )?;
    let child_pid = launcher.get_pid();
    let child_task = launcher.take_task();
    println!("child PID: {}, childTask: {}\n", child_pid, child_task);

    println!(
        "binary images: {:?}",
        get_binary_images_for_task(child_task)
    );

    let sampler = Sampler::new_with_task(child_task, Some(5.0), 0.001, true);
    sampler.start();

    thread::sleep(time::Duration::from_millis(100));

    launcher.start_execution();

    sampler.wait_until_done();
    let samples = sampler.get_samples();
    println!("samples: {:?}", samples);

    Ok(())
}

#[derive(Debug)]
struct BinaryImage {
    uuid: Option<Uuid>,
    path: String,
    name: String,
    address_range: std::ops::Range<u64>,
}

fn get_binary_images_for_task(task: mach_port_t) -> Vec<BinaryImage> {
    let mut images = Vec::new();

    autoreleasepool(|| {
        let process_description: id = unsafe { msg_send![&VMUProcessDescription_class, alloc] };
        // let task: u64 = child_task as _;
        let process_description: id =
            unsafe { msg_send![process_description, initWithTask:task getBinariesList:YES] };
        let binary_images: id = unsafe { msg_send![process_description, binaryImages] };

        // Example:
        // BinaryInfoDwarfUUIDKey = {length = 16, bytes = 0x8b4f3346832935b0a914389abc5e9260};
        // DisplayName = "libunwind.dylib";
        // ExecutablePath = "/usr/lib/system/libunwind.dylib";
        // Identifier = "libunwind.dylib";
        // IsAppleCode = 1;
        // Size = 24568;
        // SourceVersion = "35.4";
        // StartAddress = 140735077703680;
        // Version = "35.4";

        let count: NSUInteger = unsafe { NSArray::count(binary_images) };
        let exe_key: id = unsafe { msg_send![class![NSString], alloc] };
        let exe_key = unsafe { exe_key.init_str("ExecutablePath") };
        let uuid_key: id = unsafe { msg_send![class![NSString], alloc] };
        let uuid_key = unsafe { uuid_key.init_str("BinaryInfoDwarfUUIDKey") };
        let ident_key: id = unsafe { msg_send![class![NSString], alloc] };
        let ident_key = unsafe { ident_key.init_str("Identifier") };
        let start_addr_key: id = unsafe { msg_send![class![NSString], alloc] };
        let start_addr_key = unsafe { start_addr_key.init_str("StartAddress") };
        let size_key: id = unsafe { msg_send![class![NSString], alloc] };
        let size_key = unsafe { size_key.init_str("Size") };
        for i in 0..count {
            let image: id = unsafe { NSArray::objectAtIndex(binary_images, i) };
            let exe: id = unsafe { NSDictionary::objectForKey_(image, exe_key) };
            let exe_name = unsafe { CStr::from_ptr(exe.UTF8String()) };
            let path = exe_name.to_string_lossy().to_string();
            let ident: id = unsafe { NSDictionary::objectForKey_(image, ident_key) };
            let ident = unsafe { CStr::from_ptr(ident.UTF8String()) };
            let name = ident.to_string_lossy().to_string();
            let start_addr: id = unsafe { NSDictionary::objectForKey_(image, start_addr_key) };
            let start_addr: u64 = unsafe { msg_send![start_addr, unsignedLongLongValue] };
            let size: id = unsafe { NSDictionary::objectForKey_(image, size_key) };
            let size: u64 = unsafe { msg_send![size, unsignedLongLongValue] };
            let uuid: id = unsafe { NSDictionary::objectForKey_(image, uuid_key) };
            let uuid = {
                let uuid_length = unsafe { NSData::length(uuid) };
                if uuid_length == 16 {
                    let mut data = [0u8; 16];
                    unsafe { NSData::getBytes_length_(uuid, data.as_mut_ptr() as _, 16) };
                    Some(Uuid::from_bytes(data))
                } else {
                    None
                }
            };
            images.push(BinaryImage {
                uuid,
                path,
                name,
                address_range: start_addr..(start_addr + size),
            });
        }
        let _: () = unsafe { msg_send![exe_key, release] };
        let _: () = unsafe { msg_send![uuid_key, release] };
        let _: () = unsafe { msg_send![ident_key, release] };
        let _: () = unsafe { msg_send![start_addr_key, release] };
        let _: () = unsafe { msg_send![size_key, release] };

        let _: () = unsafe { msg_send![process_description, release] };
    });
    images
}

struct Sampler {
    vmu_sampler: id,
}

#[derive(Debug)]
struct Sample {
    timestamp: f64,
    thread_index: u32,
    thread_state: i32,
    frames: Vec<u64>,
}

impl Sampler {
    pub fn new_with_task(
        task: mach_port_t,
        time_limit: Option<f64>,
        interval: f64,
        all_thread_states: bool,
    ) -> Self {
        let vmu_sampler: id = unsafe { msg_send![&VMUSampler_class, alloc] };
        let vmu_sampler: id = unsafe { msg_send![vmu_sampler, initWithTask:task options:0] };
        if let Some(time_limit) = time_limit {
            let _: () = unsafe { msg_send![vmu_sampler, setTimeLimit:time_limit] };
        }
        let _: () = unsafe { msg_send![vmu_sampler, setSamplingInterval: interval] };
        let _: () = unsafe { msg_send![vmu_sampler, setRecordThreadStates: all_thread_states] };
        Sampler { vmu_sampler }
    }

    fn start(&self) {
        let _: () = unsafe { msg_send![self.vmu_sampler, start] };
    }

    fn wait_until_done(&self) {
        let _: () = unsafe { msg_send![self.vmu_sampler, waitUntilDone] };
    }

    fn get_samples(&self) -> Vec<Sample> {
        let mut samples = Vec::new();
        autoreleasepool(|| {
            let vmu_samples: id = unsafe { msg_send![self.vmu_sampler, samples] };
            let count: u64 = unsafe { msg_send![vmu_samples, count] };
            for i in 0..count {
                let backtrace: id = unsafe { msg_send![vmu_samples, objectAtIndex: i] };

                // Yikes, for the timestamps we need to get the _callstack ivar.
                let callstack: &Callstack =
                    unsafe { backtrace.as_ref().unwrap().get_ivar("_callstack") };
                let timestamp = callstack.context.t_begin / 1000000000.0;
                let thread_index = callstack.context.thread;
                let thread_state = callstack.context.run_state;
                let frame_count = callstack.length;
                let frames: Vec<_> = (0..frame_count)
                    .map(|i| unsafe { *callstack.frames.offset(i as isize) })
                    .collect();
                samples.push(Sample {
                    timestamp, thread_index, thread_state, frames
                });
            }
        });
        samples
    }
}

// struct {
//     struct {
//         double t_begin;
//         double t_end;
//         int pid;
//         unsigned int thread;
//         int run_state;
//         unsigned long long dispatch_queue_serial_num;
//     } context;
//     unsigned long long *frames;
//     unsigned long long *framePtrs;
//     unsigned int length;
// }  _callstack;
#[repr(C)]
#[derive(Debug)]
struct Callstack {
    context: CallstackContext,
    frames: *mut libc::c_ulonglong,
    frame_ptrs: *mut libc::c_ulonglong,
    length: libc::c_uint,
}

#[repr(C)]
#[derive(Debug)]
struct CallstackContext {
    t_begin: libc::c_double, // In nanoseconds since sampling start
    t_end: libc::c_double, // In nanoseconds since sampling start
    pid: libc::c_int,
    thread: libc::c_uint,
    run_state: libc::c_int,
    dispatch_queue_serial_num: libc::c_ulonglong,
}

unsafe impl objc::Encode for Callstack {
    fn encode() -> objc::Encoding {
        unsafe {
            // I got this encoding by following these steps:
            //  1. Open the Symbolication binary in Hopper.
            //  2. Look up the _callstacks ivar symbol.
            //  3. There's a list of references to that symbol, double click the
            //     last reference (which is an address without a name)
            //  4. This brings you to the "struct __objc_ivar" for the symbol,
            //     which points to an aContexttbegind string for the type.
            //     That string is the one we need.
            objc::Encoding::from_str(r#"{?="context"{?="t_begin"d"t_end"d"pid"i"thread"I"run_state"i"dispatch_queue_serial_num"Q}"frames"^Q"framePtrs"^Q"length"I}"#)
        }
    }
}
