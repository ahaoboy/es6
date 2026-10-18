//! `os` module: platform information exposed by LLRT's `llrt_os`.

use crate::runtime::Realm;
use crate::value::{JsResult, JsValue};

use super::{number_argument, set, set_method};

pub(crate) fn module(realm: &mut Realm) -> JsValue {
    let os = realm.object();
    set(
        &os,
        "EOL",
        JsValue::string(if cfg!(windows) { "\r\n" } else { "\n" }),
    );
    set(
        &os,
        "devNull",
        JsValue::string(if cfg!(windows) {
            "\\\\.\\NUL"
        } else {
            "/dev/null"
        }),
    );
    set_method(realm, &os, "platform", 0, |_, _, _| {
        Ok(JsValue::string(platform()))
    });
    set_method(realm, &os, "arch", 0, |_, _, _| Ok(JsValue::string(arch())));
    set_method(realm, &os, "type", 0, |_, _, _| {
        Ok(JsValue::string(os_type()))
    });
    set_method(realm, &os, "release", 0, |_, _, _| {
        Ok(JsValue::string(release()))
    });
    set_method(realm, &os, "version", 0, |_, _, _| {
        Ok(JsValue::string(version()))
    });
    set_method(realm, &os, "endianness", 0, |_, _, _| {
        Ok(JsValue::string(endianness()))
    });

    set_method(realm, &os, "homedir", 0, |realm, _, _| {
        Ok(JsValue::string(home_dir(realm)?))
    });
    set_method(realm, &os, "tmpdir", 0, |realm, _, _| {
        Ok(JsValue::string(
            std::env::temp_dir()
                .to_str()
                .ok_or_else(|| realm.type_error("invalid temporary directory"))?
                .to_string(),
        ))
    });
    set_method(realm, &os, "hostname", 0, |_, _, _| {
        Ok(JsValue::string(
            sysinfo::System::host_name().unwrap_or_default(),
        ))
    });
    set_method(realm, &os, "availableParallelism", 0, |_, _, _| {
        let count = std::thread::available_parallelism()
            .map(|value| value.get())
            .unwrap_or(1);
        Ok(JsValue::number(count as f64))
    });
    set_method(realm, &os, "uptime", 0, |_, _, _| {
        Ok(JsValue::number(sysinfo::System::uptime() as f64))
    });
    set_method(realm, &os, "freemem", 0, |_, _, _| {
        Ok(JsValue::number(free_memory() as f64))
    });
    set_method(realm, &os, "totalmem", 0, |_, _, _| {
        Ok(JsValue::number(total_memory() as f64))
    });
    set_method(realm, &os, "machine", 0, |_, _, _| {
        Ok(JsValue::string(sysinfo::System::cpu_arch()))
    });
    set_method(realm, &os, "loadavg", 0, |realm, _, _| {
        let load = sysinfo::System::load_average();
        let values = vec![
            JsValue::number(load.one),
            JsValue::number(load.five),
            JsValue::number(load.fifteen),
        ];
        Ok(JsValue::Object(realm.array(values)))
    });
    set_method(realm, &os, "cpus", 0, |realm, _, _| {
        let mut system = sysinfo::System::new();
        system.refresh_cpu_all();
        let mut cpus = Vec::with_capacity(system.cpus().len());
        for cpu in system.cpus() {
            let object = realm.object();
            set(&object, "model", JsValue::string(cpu.brand()));
            set(&object, "speed", JsValue::number(cpu.frequency() as f64));
            let times = realm.object();
            for name in ["user", "nice", "sys", "idle", "irq"] {
                set(&times, name, JsValue::Int(0));
            }
            set(&object, "times", JsValue::Object(times));
            cpus.push(JsValue::Object(object));
        }
        Ok(JsValue::Object(realm.array(cpus)))
    });
    set_method(realm, &os, "userInfo", 0, |realm, _, _| {
        let user = realm.object();
        set(
            &user,
            "username",
            JsValue::string(
                std::env::var("USERNAME")
                    .or_else(|_| std::env::var("USER"))
                    .unwrap_or_default(),
            ),
        );
        set(&user, "uid", JsValue::Int(0));
        set(&user, "gid", JsValue::Int(0));
        set(&user, "shell", JsValue::Null);
        set(&user, "homedir", JsValue::string(home_dir(realm)?));
        Ok(JsValue::Object(user))
    });
    set_method(realm, &os, "getPriority", 2, |realm, _, arguments| {
        let _ = number_argument(realm, arguments, 0, 0.0)?;
        Ok(JsValue::Int(0))
    });
    set_method(realm, &os, "setPriority", 2, |realm, _, arguments| {
        let _ = number_argument(realm, arguments, 0, 0.0)?;
        Ok(JsValue::Undefined)
    });
    set_method(realm, &os, "networkInterfaces", 0, |realm, _, _| {
        Ok(JsValue::Object(realm.object()))
    });
    JsValue::Object(os)
}

fn platform() -> &'static str {
    match std::env::consts::OS {
        "windows" => "win32",
        "macos" => "darwin",
        "linux" => "linux",
        "freebsd" => "freebsd",
        "android" => "android",
        other => other,
    }
}

fn arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "x86" => "ia32",
        "aarch64" => "arm64",
        "arm" => "arm",
        "riscv64" => "riscv64",
        "s390x" => "s390x",
        other => other,
    }
}

fn os_type() -> &'static str {
    match std::env::consts::OS {
        "windows" => "Windows_NT",
        "macos" => "Darwin",
        "linux" => "Linux",
        other => other,
    }
}

fn release() -> String {
    sysinfo::System::kernel_version().unwrap_or_default()
}

fn version() -> String {
    sysinfo::System::long_os_version()
        .or_else(sysinfo::System::os_version)
        .unwrap_or_default()
}

fn endianness() -> &'static str {
    if cfg!(target_endian = "little") {
        "LE"
    } else {
        "BE"
    }
}

fn home_dir(realm: &Realm) -> JsResult<String> {
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .map_err(|_| realm.type_error("home directory is not available"))
}

fn memory_info() -> (u64, u64) {
    let mut system = sysinfo::System::new();
    system.refresh_memory();
    (system.total_memory(), system.free_memory())
}

fn total_memory() -> u64 {
    memory_info().0
}

fn free_memory() -> u64 {
    memory_info().1
}
