//! # JVM Invocation Manager (`JNI_CreateJavaVM`)
//!
//! Rustプロセスが親プロセスとして主導権を握り、子スレッド内部に
//! Minecraft 1.21.11 の JVM を動的生成・管理します。

use jni::{InitArgsBuilder, JNIVersion, JavaVM, AttachGuard};
use tracing::{info, error, debug};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum JvmError {
    #[error("Failed to build JVM arguments: {0}")]
    ArgsBuildError(String),
    #[error("Failed to create Java VM: {0}")]
    CreateVmError(String),
    #[error("Failed to attach thread to JVM: {0}")]
    AttachError(String),
}

#[derive(Debug, Clone)]
pub struct JvmConfig {
    pub classpath: String,
    pub min_heap: String,
    pub max_heap: String,
    pub max_direct_memory: String,
    pub extra_jvm_args: Vec<String>,
    pub agent_path: Option<String>,
}

impl Default for JvmConfig {
    fn default() -> Self {
        Self {
            classpath: "minecraft_1.21.11.jar:libraries/*".to_string(),
            min_heap: "-Xms2G".to_string(),
            max_heap: "-Xmx8G".to_string(),
            max_direct_memory: "-XX:MaxDirectMemorySize=4G".to_string(),
            extra_jvm_args: vec![
                "-XX:+UseZGC".to_string(),
                "-XX:+ZGenerational".to_string(),
                "-XX:+UnlockExperimentalVMOptions".to_string(),
                "-Djava.library.path=./natives".to_string(),
            ],
            agent_path: None,
        }
    }
}

pub struct ManagedJvm {
    pub vm: JavaVM,
    pub config: JvmConfig,
}

impl ManagedJvm {
    pub fn launch(config: JvmConfig) -> Result<Self, JvmError> {
        info!("Preparing to launch Minecraft 1.21.11 JVM via JNI Invocation API...");
        debug!("JVM Classpath: {}", config.classpath);
        debug!("Memory Config: {} / {} / {}", config.min_heap, config.max_heap, config.max_direct_memory);

        let cp_arg = format!("-Djava.class.path={}", config.classpath);
        let mut builder = InitArgsBuilder::new()
            .version(JNIVersion::V8)
            .option(&cp_arg)
            .option(&config.min_heap)
            .option(&config.max_heap)
            .option(&config.max_direct_memory);

        for arg in &config.extra_jvm_args {
            builder = builder.option(arg);
        }

        builder = builder.option("-Drsift.loader.version=0.1.0-alpha");
        builder = builder.option("-Drsift.minecraft.version=1.21.11");
        builder = builder.option("-Drsift.native.injection=true");
        builder = builder.option("-Drsift.compute.parity=strict");

        let mut agent_opts: Vec<String> = Vec::new();
        if let Some(ref ap) = config.agent_path {
            agent_opts.push(format!("-agentpath:{}", ap));
        } else if let Some(p) = Self::find_agent_library() {
            agent_opts.push(format!("-agentpath:{}", p));
        }
        for arg in &agent_opts {
            info!("Registering JVMTI agent: {}", arg);
            builder = builder.option(arg);
        }

        let jni_args = builder.build().map_err(|e| JvmError::ArgsBuildError(format!("{:?}", e)))?;

        info!("Calling JNI_CreateJavaVM... (Rust thread takes mastery of JVM memory space)");
        let vm = JavaVM::new(jni_args).map_err(|e| {
            let home = std::env::var("JAVA_HOME").unwrap_or_else(|_| "?".into());
            error!(
                "JNI_CreateJavaVM failed (JAVA_HOME={}): {:?}",
                home, e
            );
            JvmError::CreateVmError(format!(
                "JAVA_HOME={home} — {e:?}. Minecraft 1.21 には Java 21+ が必要です。"
            ))
        })?;

        info!("JVM dynamically created inside Rust process! Rsift holds absolute mastery.");
        Ok(Self { vm, config })
    }

    fn find_agent_library() -> Option<String> {
        let candidates = [
            "target/release/rsift_jvm.dll",
            "target/debug/rsift_jvm.dll",
            "./rsift_jvm.dll",
        ];
        for path in &candidates {
            if std::path::Path::new(path).exists() {
                return Some(path.to_string());
            }
        }
        if let Ok(app) = std::env::var("APPDATA") {
            let versions = std::path::Path::new(&app).join(".minecraft").join("versions");
            if let Ok(read) = std::fs::read_dir(&versions) {
                let mut ids: Vec<_> = read
                    .filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| n.starts_with("rsift-loader-"))
                    .collect();
                ids.sort();
                if let Some(id) = ids.last() {
                    let dll = versions.join(id).join("rsift_jvm.dll");
                    if dll.is_file() {
                        return Some(dll.display().to_string());
                    }
                }
            }
        }
        None
    }

    pub fn attach_current_thread(&self) -> Result<AttachGuard<'_>, JvmError> {
        self.vm.attach_current_thread().map_err(|e| {
            JvmError::AttachError(format!("{:?}", e))
        })
    }

    pub fn start_minecraft_client(&self, game_args: &[&str]) -> Result<(), String> {
        let mut env = self.attach_current_thread().map_err(|e| e.to_string())?;

        info!("Locating net/minecraft/client/main/Main class...");
        let main_class = env.find_class("net/minecraft/client/main/Main").map_err(|e| {
            format!("Failed to find Minecraft Main class: {:?}", e)
        })?;

        let string_class = env.find_class("java/lang/String").map_err(|e| format!("{:?}", e))?;
        let empty_string = env.new_string("").map_err(|e| format!("{:?}", e))?;
        let args_array = env.new_object_array(game_args.len() as i32, string_class, empty_string)
            .map_err(|e| format!("{:?}", e))?;

        for (i, arg) in game_args.iter().enumerate() {
            let j_str = env.new_string(arg).map_err(|e| format!("{:?}", e))?;
            env.set_object_array_element(&args_array, i as i32, j_str).map_err(|e| format!("{:?}", e))?;
        }

        info!("Invoking net.minecraft.client.main.Main.main(String[] args) via JNI...");
        env.call_static_method(
            &main_class,
            "main",
            "([Ljava/lang/String;)V",
            &[jni::objects::JValue::Object(&args_array)],
        ).map_err(|e| format!("Minecraft main execution threw an exception: {:?}", e))?;

        info!("Minecraft client process finished gracefully.");
        Ok(())
    }
}
