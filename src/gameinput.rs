#![allow(unsafe_op_in_unsafe_fn)]

use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

// ---- HRESULT ---------------------------------------------------------------

type HRESULT = i32;

fn succeeded(hr: HRESULT) -> bool {
    hr >= 0
}

// ---- GameInputKind ---------------------------------------------------------

const GAMEINPUT_KIND_GAMEPAD: u32 = 0x00040000;

// ---- GameInputSystemButtons ------------------------------------------------

const GUIDE: u32 = 0x00000001;

// ---- GameInputGamepadButtons -----------------------------------------------

const GAMEPAD_A: u32 = 0x00000004;
const GAMEPAD_DPAD_UP: u32 = 0x00000040;
const GAMEPAD_DPAD_DOWN: u32 = 0x00000080;
const GAMEPAD_DPAD_LEFT: u32 = 0x00000100;
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
struct GameInputGamepadState {
    buttons: u32,
    left_trigger: f32,
    right_trigger: f32,
    left_thumbstick_x: f32,
    left_thumbstick_y: f32,
    right_thumbstick_x: f32,
    right_thumbstick_y: f32,
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
const SLOT_GET_CURRENT_READING: usize = 4;
const SLOT_REGISTER_SYSTEM_BUTTON_CALLBACK: usize = 10;
const SLOT_SET_FOCUS_POLICY: usize = 21;

const SLOT_READING_RELEASE: usize = 2;
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
/// Returns the buttons bitmask, or 0 if no gamepad is connected / no reading.
unsafe fn get_current_gamepad_buttons(game_input: ComPtr) -> u32 {
    // GetCurrentReading(inputKind, device, **reading) -> HRESULT
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
        return 0;
    }

    // GetGamepadState(*state) -> bool
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

    if ok { state.buttons } else { 0 }
}

// ---- GameInputCreate -------------------------------------------------------

#[link(name = "GameInput")]
unsafe extern "system" {
    fn GameInputCreate(out: *mut ComPtr) -> HRESULT;
}

// ---- Atomic state ----------------------------------------------------------
//
// Guide: rising-edge counter (callback-driven, background thread).
// D-pad + A: edge-detected via polling in drain_gamepad_events(), called
// every frame from the UI thread. We track the previous button state in
// a separate atomic so we can detect rising edges without a mutex.

static GUIDE_PRESSED: AtomicU32 = AtomicU32::new(0);

// Each of these counts rising edges detected during polling.
static DPAD_UP_PRESSED: AtomicU32 = AtomicU32::new(0);
static DPAD_DOWN_PRESSED: AtomicU32 = AtomicU32::new(0);
static DPAD_LEFT_PRESSED: AtomicU32 = AtomicU32::new(0);
static DPAD_RIGHT_PRESSED: AtomicU32 = AtomicU32::new(0);
static A_PRESSED: AtomicU32 = AtomicU32::new(0);

// Previous buttons seen during polling (used for edge detection).
static PREV_GAMEPAD_BUTTONS: AtomicU32 = AtomicU32::new(0);

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
            let mut game_input: ComPtr = std::ptr::null_mut();
            let hr = GameInputCreate(&mut game_input);
            if !succeeded(hr) {
                return Err(hr);
            }

            set_focus_policy(game_input, FOCUS_EXCLUSIVE_FOREGROUND_GUIDE);

            let mut token: GameInputCallbackToken = 0;
            let hr = register_system_button_callback(
                game_input,
                GUIDE,
                std::ptr::null_mut(),
                system_button_callback,
                &mut token,
            );
            if !succeeded(hr) {
                com_release(game_input);
                return Err(hr);
            }

            Ok(Self {
                game_input,
                _token: token,
            })
        }
    }

    /// Drain Guide rising-edge presses since last call.
    pub fn drain_guide_presses(&self) -> u32 {
        GUIDE_PRESSED.swap(0, Ordering::Relaxed)
    }

    /// Poll the gamepad, detect rising edges for D-pad and A, and return
    /// a [`GamepadEvents`] with counts for each button since last call.
    /// Call once per frame.
    pub fn drain_gamepad_events(&self) -> GamepadEvents {
        let buttons = unsafe { get_current_gamepad_buttons(self.game_input) };
        let prev = PREV_GAMEPAD_BUTTONS.swap(buttons, Ordering::Relaxed);
        let rose = buttons & !prev; // bits that just became 1

        fn edge(rose: u32, mask: u32, counter: &AtomicU32) -> u32 {
            if rose & mask != 0 {
                counter.fetch_add(1, Ordering::Relaxed);
            }
            counter.swap(0, Ordering::Relaxed)
        }

        GamepadEvents {
            dpad_up: edge(rose, GAMEPAD_DPAD_UP, &DPAD_UP_PRESSED),
            dpad_down: edge(rose, GAMEPAD_DPAD_DOWN, &DPAD_DOWN_PRESSED),
            dpad_left: edge(rose, GAMEPAD_DPAD_LEFT, &DPAD_LEFT_PRESSED),
            dpad_right: edge(rose, GAMEPAD_DPAD_RIGHT, &DPAD_RIGHT_PRESSED),
            a: edge(rose, GAMEPAD_A, &A_PRESSED),
        }
    }
}

impl Drop for GameInputHandle {
    fn drop(&mut self) {
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
}
