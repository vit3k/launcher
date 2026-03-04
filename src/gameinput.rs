#![allow(unsafe_op_in_unsafe_fn)]

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

// ---- XInput ----------------------------------------------------------------
//
// We use XInput as the primary input source because GameInput requires the
// GameInput Service to be running and a compatible driver, whereas XInput
// works out of the box for any Xbox-compatible controller.
//
// XInput button bitmasks (XINPUT_GAMEPAD):
//   VIEW  (Back)   = 0x0020
//   B              = 0x2000
//   A              = 0x1000
//   X              = 0x4000
//   DPAD_UP        = 0x0001
//   DPAD_DOWN      = 0x0002
//   DPAD_LEFT      = 0x0004
//   DPAD_RIGHT     = 0x0008

use windows::Win32::UI::Input::XboxController::{
    XINPUT_GAMEPAD_A, XINPUT_GAMEPAD_B, XINPUT_GAMEPAD_BACK, XINPUT_GAMEPAD_DPAD_DOWN,
    XINPUT_GAMEPAD_DPAD_LEFT, XINPUT_GAMEPAD_DPAD_RIGHT, XINPUT_GAMEPAD_DPAD_UP, XINPUT_GAMEPAD_X,
    XINPUT_STATE, XInputGetState,
};

const XINPUT_ERROR_DEVICE_NOT_CONNECTED: u32 = 1167;

// ---- HRESULT ---------------------------------------------------------------

type HRESULT = i32;

fn succeeded(hr: HRESULT) -> bool {
    hr >= 0
}

// ---- GameInputKind ---------------------------------------------------------

#[allow(dead_code)]
const GAMEINPUT_KIND_GAMEPAD: u32 = 0x00040000;

// ---- GameInputSystemButtons ------------------------------------------------

const GUIDE: u32 = 0x00000001;

// ---- GameInputGamepadButtons -----------------------------------------------

#[allow(dead_code)]
const GAMEPAD_A: u32 = 0x00000004;
#[allow(dead_code)]
const GAMEPAD_B: u32 = 0x00000002;
#[allow(dead_code)]
const GAMEPAD_X: u32 = 0x00000010;
#[allow(dead_code)]
const GAMEPAD_VIEW: u32 = 0x00000020;
#[allow(dead_code)]
const GAMEPAD_DPAD_UP: u32 = 0x00000040;
#[allow(dead_code)]
const GAMEPAD_DPAD_DOWN: u32 = 0x00000080;
#[allow(dead_code)]
const GAMEPAD_DPAD_LEFT: u32 = 0x00000100;
#[allow(dead_code)]
const GAMEPAD_DPAD_RIGHT: u32 = 0x00000200;

// ---- GameInputCallbackToken ------------------------------------------------

type GameInputCallbackToken = u64;

// ---- GameInputGamepadState -------------------------------------------------
//
// Must match the C struct layout exactly:
//   GameInputGamepadButtons buttons;   (u32)
//   float leftTrigger;
//   float rightTrigger;
//   float leftThumbstickX;
//   float leftThumbstickY;
//   float rightThumbstickX;
//   float rightThumbstickY;

#[repr(C)]
#[allow(dead_code)]
struct GameInputGamepadState {
    buttons: u32,
    left_trigger: f32,
    right_trigger: f32,
    left_thumbstick_x: f32,
    left_thumbstick_y: f32,
    right_thumbstick_x: f32,
    right_thumbstick_y: f32,
}

#[allow(dead_code)]
struct GamepadReading {
    buttons: u32,
}

// ---- COM vtable dispatch ---------------------------------------------------
//
// IGameInput vtable slots (GameInput.h, SDK 10.0.26100.0):
//   [0]  QueryInterface
//   [1]  AddRef
//   [2]  Release
//   [3]  GetCurrentTimestamp
//   [4]  GetCurrentReading
//   [5]  GetNextReading
//   [6]  GetPreviousReading
//   [7]  GetTemporalReading
//   [8]  RegisterReadingCallback
//   [9]  RegisterDeviceCallback
//   [10] RegisterSystemButtonCallback
//   [11] RegisterKeyboardLayoutCallback
//   [12] StopCallback
//   [13] UnregisterCallback
//   ...
//   [21] SetFocusPolicy
//
// IGameInputReading vtable slots:
//   [0]  QueryInterface
//   [1]  AddRef
//   [2]  Release
//   [3]  GetInputKind
//   [4]  GetSequenceNumber
//   [5]  GetTimestamp
//   [6]  GetDevice
//   [7]  GetRawReport
//   [8]  GetControllerAxisCount
//   [9]  GetControllerAxisState
//   [10] GetControllerButtonCount
//   [11] GetControllerButtonState
//   [12] GetControllerSwitchCount
//   [13] GetControllerSwitchState
//   [14] GetKeyCount
//   [15] GetKeyState
//   [16] GetMouseState
//   [17] GetTouchCount
//   [18] GetTouchState
//   [19] GetMotionState
//   [20] GetArcadeStickState
//   [21] GetFlightStickState
//   [22] GetGamepadState
//   [23] GetRacingWheelState
//   [24] GetUiNavigationState

type ComPtr = *mut *mut c_void;

const SLOT_RELEASE: usize = 2;
#[allow(dead_code)]
const SLOT_GET_CURRENT_READING: usize = 4;
const SLOT_REGISTER_SYSTEM_BUTTON_CALLBACK: usize = 10;
const SLOT_SET_FOCUS_POLICY: usize = 21;

#[allow(dead_code)]
const SLOT_READING_RELEASE: usize = 2;
#[allow(dead_code)]
const SLOT_READING_GET_GAMEPAD_STATE: usize = 22;

const FOCUS_EXCLUSIVE_FOREGROUND_GUIDE: u32 = 0x00000008;

unsafe fn vtable_fn<F: Copy>(obj: ComPtr, slot: usize) -> F {
    let vtable = *obj as *mut *mut c_void;
    let fn_ptr = vtable.add(slot).read();
    std::mem::transmute_copy(&fn_ptr)
}

unsafe fn com_release(obj: ComPtr) {
    let f: unsafe extern "system" fn(ComPtr) -> u32 = vtable_fn(obj, SLOT_RELEASE);
    f(obj);
}

#[allow(dead_code)]
unsafe fn reading_release(reading: ComPtr) {
    let f: unsafe extern "system" fn(ComPtr) -> u32 = vtable_fn(reading, SLOT_READING_RELEASE);
    f(reading);
}

unsafe fn set_focus_policy(game_input: ComPtr, policy: u32) {
    let f: unsafe extern "system" fn(ComPtr, u32) = vtable_fn(game_input, SLOT_SET_FOCUS_POLICY);
    f(game_input, policy);
}

unsafe fn register_system_button_callback(
    game_input: ComPtr,
    button_filter: u32,
    context: *mut c_void,
    callback: unsafe extern "system" fn(
        token: GameInputCallbackToken,
        context: *mut c_void,
        device: ComPtr,
        timestamp: u64,
        current_buttons: u32,
        previous_buttons: u32,
    ),
    out_token: *mut GameInputCallbackToken,
) -> HRESULT {
    let f: unsafe extern "system" fn(
        ComPtr,
        ComPtr,
        u32,
        *mut c_void,
        unsafe extern "system" fn(GameInputCallbackToken, *mut c_void, ComPtr, u64, u32, u32),
        *mut GameInputCallbackToken,
    ) -> HRESULT = vtable_fn(game_input, SLOT_REGISTER_SYSTEM_BUTTON_CALLBACK);
    f(
        game_input,
        std::ptr::null_mut(),
        button_filter,
        context,
        callback,
        out_token,
    )
}

/// Poll the latest gamepad reading from IGameInput.
/// Returns buttons + right trigger, or zeroed defaults if no gamepad / no reading.
// Counter so we only log the "no reading" case occasionally, not every frame.
#[allow(dead_code)]
static NO_READING_LOG_COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

#[allow(dead_code)]
unsafe fn get_current_gamepad_reading(game_input: ComPtr) -> GamepadReading {
    let get_reading: unsafe extern "system" fn(
        ComPtr,      // this
        u32,         // inputKind
        ComPtr,      // device (NULL = any)
        *mut ComPtr, // out reading
    ) -> HRESULT = vtable_fn(game_input, SLOT_GET_CURRENT_READING);

    let mut reading: ComPtr = std::ptr::null_mut();
    let hr = get_reading(
        game_input,
        GAMEINPUT_KIND_GAMEPAD,
        std::ptr::null_mut(),
        &mut reading,
    );
    if !succeeded(hr) || reading.is_null() {
        // Log once every ~300 frames so we don't spam the file.
        let n = NO_READING_LOG_COUNTER.fetch_add(1, Ordering::Relaxed);
        if n % 300 == 0 {
            log(&format!(
                "[poll] GetCurrentReading returned no gamepad reading (hr=0x{hr:08X}) frame={n}"
            ));
        }
        return GamepadReading { buttons: 0 };
    }

    let get_gamepad: unsafe extern "system" fn(ComPtr, *mut GameInputGamepadState) -> bool =
        vtable_fn(reading, SLOT_READING_GET_GAMEPAD_STATE);

    let mut state = GameInputGamepadState {
        buttons: 0,
        left_trigger: 0.0,
        right_trigger: 0.0,
        left_thumbstick_x: 0.0,
        left_thumbstick_y: 0.0,
        right_thumbstick_x: 0.0,
        right_thumbstick_y: 0.0,
    };

    let ok = get_gamepad(reading, &mut state);
    reading_release(reading);

    if ok {
        GamepadReading {
            buttons: state.buttons,
        }
    } else {
        GamepadReading { buttons: 0 }
    }
}

// ---- GameInputCreate -------------------------------------------------------

#[link(name = "GameInput")]
unsafe extern "system" {
    fn GameInputCreate(out: *mut ComPtr) -> HRESULT;
}

fn log(msg: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("gameinput_debug.log")
    {
        let _ = writeln!(f, "{msg}");
    }
}

// ---- Atomic state ----------------------------------------------------------
//
// Guide: rising-edge counter (callback-driven, background thread).
// D-pad + A: edge-detected via polling in drain_gamepad_events(), called
// every frame from the UI thread. We track the previous button state in
// a separate atomic so we can detect rising edges without a mutex.

static GUIDE_PRESSED: AtomicU32 = AtomicU32::new(0);

// Combo (View + X + RightTrigger) rising-edge counter (polling-driven).
static COMBO_PRESSED: AtomicU32 = AtomicU32::new(0);
// 1 when the combo was fully held last poll, 0 when it was not.
static PREV_COMBO_HELD: AtomicU32 = AtomicU32::new(0);

// Each of these counts rising edges detected during polling.
static DPAD_UP_PRESSED: AtomicU32 = AtomicU32::new(0);
static DPAD_DOWN_PRESSED: AtomicU32 = AtomicU32::new(0);
static DPAD_LEFT_PRESSED: AtomicU32 = AtomicU32::new(0);
static DPAD_RIGHT_PRESSED: AtomicU32 = AtomicU32::new(0);
static A_PRESSED: AtomicU32 = AtomicU32::new(0);
static X_PRESSED: AtomicU32 = AtomicU32::new(0);

// Previous buttons seen during polling (used for edge detection).
static PREV_GAMEPAD_BUTTONS: AtomicU32 = AtomicU32::new(0);

// Set to true by the background polling thread to signal it should stop.
static STOP_POLL: AtomicBool = AtomicBool::new(false);

// Optional callback invoked by the poll thread when the combo fires, used to
// wake up the UI thread. Stored as a boxed fn so gameinput has no eframe dep.
static REPAINT_CB: Mutex<Option<Arc<dyn Fn() + Send + Sync>>> = Mutex::new(None);

// ---- System-button callback ------------------------------------------------

unsafe extern "system" fn system_button_callback(
    _token: GameInputCallbackToken,
    _context: *mut c_void,
    _device: ComPtr,
    _timestamp: u64,
    current_buttons: u32,
    previous_buttons: u32,
) {
    if current_buttons & GUIDE != 0 && previous_buttons & GUIDE == 0 {
        GUIDE_PRESSED.fetch_add(1, Ordering::Relaxed);
    }
}

// ---- XInput background polling --------------------------------------------
//
// Polls all four XInput slots at ~120 Hz. Drives combo detection and all
// gamepad button edge counters. Works without the GameInput Service.

fn xinput_poll_loop() {
    log("[xinput_poll_loop] started");
    loop {
        if STOP_POLL.load(Ordering::Relaxed) {
            break;
        }

        // Try all four controller slots; use the first connected one.
        let mut buttons: u32 = 0;
        for user in 0u32..4 {
            let mut state = XINPUT_STATE::default();
            let err = unsafe { XInputGetState(user, &mut state) };
            if err == 0 {
                buttons = state.Gamepad.wButtons.0 as u32;
                break;
            } else if err != XINPUT_ERROR_DEVICE_NOT_CONNECTED {
                log(&format!("[xinput] XInputGetState slot {user} err={err}"));
            }
        }

        let prev = PREV_GAMEPAD_BUTTONS.swap(buttons, Ordering::Relaxed);
        let rose = buttons & !prev;

        if buttons != prev {
            log(&format!(
                "[xinput] buttons=0x{buttons:04X}  VIEW={} B={} X={} A={}",
                (buttons & XINPUT_GAMEPAD_BACK.0 as u32) != 0,
                (buttons & XINPUT_GAMEPAD_B.0 as u32) != 0,
                (buttons & XINPUT_GAMEPAD_X.0 as u32) != 0,
                (buttons & XINPUT_GAMEPAD_A.0 as u32) != 0,
            ));
        }

        // ── View + B combo ───────────────────────────────────────────────
        let combo_mask = (XINPUT_GAMEPAD_BACK.0 | XINPUT_GAMEPAD_B.0) as u32;
        let combo_held = (buttons & combo_mask) == combo_mask;
        let prev_combo = PREV_COMBO_HELD.swap(combo_held as u32, Ordering::Relaxed);
        if combo_held && prev_combo == 0 {
            log("[xinput] combo FIRED (View+B rising edge)");
            COMBO_PRESSED.fetch_add(1, Ordering::Relaxed);
            if let Ok(guard) = REPAINT_CB.lock() {
                if let Some(cb) = guard.as_ref() {
                    cb();
                }
            }
        }

        // Edge-detect individual buttons.
        fn edge(rose: u32, mask: u32, counter: &AtomicU32) {
            if rose & mask != 0 {
                counter.fetch_add(1, Ordering::Relaxed);
            }
        }
        edge(rose, XINPUT_GAMEPAD_DPAD_UP.0 as u32, &DPAD_UP_PRESSED);
        edge(rose, XINPUT_GAMEPAD_DPAD_DOWN.0 as u32, &DPAD_DOWN_PRESSED);
        edge(rose, XINPUT_GAMEPAD_DPAD_LEFT.0 as u32, &DPAD_LEFT_PRESSED);
        edge(
            rose,
            XINPUT_GAMEPAD_DPAD_RIGHT.0 as u32,
            &DPAD_RIGHT_PRESSED,
        );
        edge(rose, XINPUT_GAMEPAD_A.0 as u32, &A_PRESSED);
        edge(rose, XINPUT_GAMEPAD_X.0 as u32, &X_PRESSED);

        std::thread::sleep(std::time::Duration::from_millis(8)); // ~120 Hz
    }
    log("[xinput_poll_loop] stopped");
}

// ---- GameInput background polling (kept for Guide button via callback) -----

fn gameinput_poll_loop(_game_input: ComPtr) {
    log("[gameinput_poll_loop] started");
    while !STOP_POLL.load(Ordering::Relaxed) {
        // GameInput polling is only used for the Guide system button callback
        // which is registered in init(). Nothing extra to poll here.
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    log("[gameinput_poll_loop] stopped");
}

// ---- Public handle ---------------------------------------------------------

pub struct GameInputHandle {
    game_input: ComPtr,
    _token: GameInputCallbackToken,
}

// SAFETY: ComPtr is a raw pointer to a COM object that GameInput guarantees
// is safe to call from any thread (it is internally synchronized).
unsafe impl Send for GameInputHandle {}
unsafe impl Sync for GameInputHandle {}

impl GameInputHandle {
    pub fn init() -> Result<Self, HRESULT> {
        unsafe {
            log("[init] calling GameInputCreate");
            let mut game_input: ComPtr = std::ptr::null_mut();
            let hr = GameInputCreate(&mut game_input);
            if !succeeded(hr) {
                log(&format!("[init] GameInputCreate FAILED hr=0x{hr:08X}"));
                return Err(hr);
            }
            log(&format!("[init] GameInputCreate OK ptr={game_input:?}"));

            set_focus_policy(game_input, FOCUS_EXCLUSIVE_FOREGROUND_GUIDE);
            log("[init] SetFocusPolicy done");

            let mut token: GameInputCallbackToken = 0;
            let hr = register_system_button_callback(
                game_input,
                GUIDE,
                std::ptr::null_mut(),
                system_button_callback,
                &mut token,
            );
            if !succeeded(hr) {
                log(&format!(
                    "[init] RegisterSystemButtonCallback FAILED hr=0x{hr:08X}"
                ));
                com_release(game_input);
                return Err(hr);
            }
            log("[init] RegisterSystemButtonCallback OK");

            // Spawn the GameInput background thread (Guide callback keepalive).
            static POLL_PTR: std::sync::atomic::AtomicUsize =
                std::sync::atomic::AtomicUsize::new(0);
            POLL_PTR.store(game_input as usize, Ordering::Relaxed);
            STOP_POLL.store(false, Ordering::Relaxed);
            std::thread::Builder::new()
                .name("gameinput-poll".into())
                .spawn(|| {
                    let ptr = POLL_PTR.load(Ordering::Relaxed) as ComPtr;
                    gameinput_poll_loop(ptr);
                })
                .expect("failed to spawn gameinput poll thread");
            log("[init] gameinput poll thread spawned");

            // Spawn the XInput poll thread — drives combo + all gamepad events.
            std::thread::Builder::new()
                .name("xinput-poll".into())
                .spawn(xinput_poll_loop)
                .expect("failed to spawn xinput poll thread");
            log("[init] xinput poll thread spawned");

            Ok(Self {
                game_input,
                _token: token,
            })
        }
    }

    /// Register a callback that the background poll thread will invoke when the
    /// combo fires, so the UI thread can be woken up. Call once at startup.
    pub fn set_repaint_callback(&self, cb: Arc<dyn Fn() + Send + Sync>) {
        if let Ok(mut guard) = REPAINT_CB.lock() {
            *guard = Some(cb);
        }
    }

    /// Drain Guide rising-edge presses since last call.
    pub fn drain_guide_presses(&self) -> u32 {
        GUIDE_PRESSED.swap(0, Ordering::Relaxed)
    }

    /// Drain View+B combo rising-edge presses since last call.
    pub fn drain_combo_presses(&self) -> u32 {
        COMBO_PRESSED.swap(0, Ordering::Relaxed)
    }

    /// Drain edge-detected button events accumulated by the background thread.
    /// Call once per frame from the UI thread.
    pub fn drain_gamepad_events(&self) -> GamepadEvents {
        fn take(counter: &AtomicU32) -> u32 {
            counter.swap(0, Ordering::Relaxed)
        }
        GamepadEvents {
            dpad_up: take(&DPAD_UP_PRESSED),
            dpad_down: take(&DPAD_DOWN_PRESSED),
            dpad_left: take(&DPAD_LEFT_PRESSED),
            dpad_right: take(&DPAD_RIGHT_PRESSED),
            a: take(&A_PRESSED),
            x: take(&X_PRESSED),
        }
    }
}

impl Drop for GameInputHandle {
    fn drop(&mut self) {
        STOP_POLL.store(true, Ordering::Relaxed);
        unsafe {
            com_release(self.game_input);
        }
    }
}

// ---- GamepadEvents ---------------------------------------------------------

/// Rising-edge counts for each button of interest, drained each frame.
/// A value > 0 means that button was pressed at least once since last poll.
#[derive(Default)]
pub struct GamepadEvents {
    pub dpad_up: u32,
    pub dpad_down: u32,
    pub dpad_left: u32,
    pub dpad_right: u32,
    pub a: u32,
    pub x: u32,
}
