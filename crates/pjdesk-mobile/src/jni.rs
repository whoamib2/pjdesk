//! JNI entry points. The Kotlin class is `us.spader.pjdesk.core.Native`.

use std::ffi::c_void;
use std::sync::{Arc, OnceLock};

use jni::objects::{JByteArray, JClass, JObject, JString, JValue};
use jni::sys::{jboolean, jint, jlong, JNI_VERSION_1_6};
use jni::{JNIEnv, JavaVM};

use crate::engine::{Callbacks, Engine};
use crate::gesture::{self, Frame, View};

static VM: OnceLock<JavaVM> = OnceLock::new();

struct JavaCb {
    listener: jni::objects::GlobalRef,
}

impl Callbacks for JavaCb {
    fn on_event(&self, json: &str) {
        let _ = with_env(|env| {
            let text = env.new_string(json)?;
            env.call_method(&self.listener, "onEvent", "(Ljava/lang/String;)V", &[JValue::Object(text.as_ref())])?;
            Ok(())
        });
    }

    fn on_video(&self, data: &[u8], width: u32, height: u32, keyframe: bool) {
        let _ = with_env(|env| {
            let bytes = env.byte_array_from_slice(data)?;
            env.call_method(
                &self.listener,
                "onVideo",
                "([BIIZ)V",
                &[
                    JValue::Object(bytes.as_ref()),
                    JValue::Int(width as jint),
                    JValue::Int(height as jint),
                    JValue::Bool(keyframe as jboolean),
                ],
            )?;
            Ok(())
        });
    }
}

fn with_env(f: impl FnOnce(&mut JNIEnv) -> jni::errors::Result<()>) -> jni::errors::Result<()> {
    let vm = VM.get().ok_or(jni::errors::Error::NullPtr("JavaVM"))?;
    let mut guard = vm.attach_current_thread()?;
    let result = f(&mut guard);
    if guard.exception_check().unwrap_or(false) {
        let _ = guard.exception_describe();
        let _ = guard.exception_clear();
    }
    result
}

#[no_mangle]
pub extern "system" fn JNI_OnLoad(vm: JavaVM, _: *mut c_void) -> jint {
    let _ = tracing_subscriber::fmt().with_max_level(tracing::Level::INFO).try_init();
    let _ = VM.set(vm);
    JNI_VERSION_1_6
}

fn engine<'a>(handle: jlong) -> &'a Engine {
    unsafe { &*(handle as *const Engine) }
}

#[no_mangle]
pub extern "system" fn Java_us_spader_pjdesk_core_Native_nativeCreate(
    mut env: JNIEnv,
    _class: JClass,
    dir: JString,
    android_id: JString,
    hostname: JString,
    listener: JObject,
) -> jlong {
    let dir: String = env.get_string(&dir).map(|s| s.into()).unwrap_or_default();
    let android_id: String = env.get_string(&android_id).map(|s| s.into()).unwrap_or_default();
    let hostname: String = env.get_string(&hostname).map(|s| s.into()).unwrap_or_default();
    let listener = match env.new_global_ref(listener) {
        Ok(r) => r,
        Err(_) => return 0,
    };
    let cb = Arc::new(JavaCb { listener });
    match Engine::start(dir.into(), android_id, hostname, cb) {
        Ok(engine) => Box::into_raw(Box::new(engine)) as jlong,
        Err(e) => {
            tracing::error!("engine start failed: {e:#}");
            0
        }
    }
}

#[no_mangle]
pub extern "system" fn Java_us_spader_pjdesk_core_Native_nativeDestroy(_env: JNIEnv, _class: JClass, handle: jlong) {
    if handle == 0 {
        return;
    }
    let engine = unsafe { Box::from_raw(handle as *mut Engine) };
    engine.command(r#"{"cmd":"shutdown"}"#);
    drop(engine);
}

#[no_mangle]
pub extern "system" fn Java_us_spader_pjdesk_core_Native_nativeCommand(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    json: JString,
) {
    if handle == 0 {
        return;
    }
    let json: String = env.get_string(&json).map(|s| s.into()).unwrap_or_default();
    engine(handle).command(&json);
}

#[no_mangle]
pub extern "system" fn Java_us_spader_pjdesk_core_Native_nativeSubmitVideo(
    env: JNIEnv,
    _class: JClass,
    handle: jlong,
    data: JByteArray,
    width: jint,
    height: jint,
    keyframe: jboolean,
    encode_us: jint,
) {
    if handle == 0 {
        return;
    }
    let bytes = env.convert_byte_array(data).unwrap_or_default();
    engine(handle).submit_video(
        bytes,
        width.max(0) as u16,
        height.max(0) as u16,
        keyframe != 0,
        encode_us.max(0) as u32,
    );
}

/// Packed result: high 32 bits are x, low 32 bits are y. `-1` is outside the picture.
#[no_mangle]
pub extern "system" fn Java_us_spader_pjdesk_core_Native_nativeViewToNorm(
    _env: JNIEnv,
    _class: JClass,
    frame_w: jint,
    frame_h: jint,
    view_w: jint,
    view_h: jint,
    x: jni::sys::jfloat,
    y: jni::sys::jfloat,
) -> jlong {
    let placed = gesture::letterbox(
        Frame { width: frame_w.max(0) as u32, height: frame_h.max(0) as u32 },
        View { width: view_w.max(0) as u32, height: view_h.max(0) as u32 },
    );
    match gesture::view_to_norm(placed, x, y) {
        Some((nx, ny)) => ((nx as jlong) << 32) | ny as jlong,
        None => -1,
    }
}

/// `[x, y, width, height]` of the letterboxed picture, or an empty array.
#[no_mangle]
pub extern "system" fn Java_us_spader_pjdesk_core_Native_nativeLetterbox<'a>(
    env: JNIEnv<'a>,
    _class: JClass<'a>,
    frame_w: jint,
    frame_h: jint,
    view_w: jint,
    view_h: jint,
) -> JObject<'a> {
    let placed = gesture::letterbox(
        Frame { width: frame_w.max(0) as u32, height: frame_h.max(0) as u32 },
        View { width: view_w.max(0) as u32, height: view_h.max(0) as u32 },
    );
    let values = [placed.x, placed.y, placed.width as i32, placed.height as i32];
    env.new_int_array(4)
        .and_then(|arr| {
            env.set_int_array_region(&arr, 0, &values)?;
            Ok(arr.into())
        })
        .unwrap_or_else(|_| JObject::null())
}
