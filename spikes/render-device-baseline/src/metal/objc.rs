//! The Objective-C runtime as the Metal module reaches it: messages sent
//! through `objc_msgSend` cast to the exact function type of each method,
//! reference counts, autorelease pools and strings.
//!
//! A message is sent only once the receiver is known to exist and to answer
//! the selector, so that a mistake in a selector or an object that was never
//! created is an error naming both instead of the end of the process.
//! Throwaway, never product code.

use std::ffi::{CStr, c_char, c_void};
use std::io::Write;

use crate::loader::{Library, c_string};

/// An Objective-C object or class, as a message receives it.
pub type Id = *mut c_void;
/// A registered selector.
pub type Sel = *mut c_void;

#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    fn objc_getClass(name: *const c_char) -> Id;
    fn sel_registerName(name: *const c_char) -> Sel;
    fn object_getClassName(object: Id) -> *const c_char;
    fn objc_autoreleasePoolPush() -> *mut c_void;
    fn objc_autoreleasePoolPop(pool: *mut c_void);
}

#[link(name = "objc", kind = "dylib")]
unsafe extern "C-unwind" {
    /// The dispatcher of every message. It has no signature of its own: it is
    /// entered as the method it dispatches to would be.
    pub fn objc_msgSend();
}

/// `[receiver selector: argument ...]`, written `send!(ReturnType; receiver,
/// "selector:", ArgumentType: argument, ...)`. The types are those of the
/// method's declaration: `objc_msgSend` is never called as a variadic
/// function. Evaluates to `Result<ReturnType, String>`: the error is a nil
/// receiver or a selector the receiver does not answer. The selector is a
/// literal, terminated here at compile time, so that a send allocates nothing.
macro_rules! send {
    ($result:ty; $receiver:expr, $name:literal $(, $kind:ty : $argument:expr)* $(,)?) => {{
        let receiver: $crate::metal::objc::Id = $receiver;
        match $crate::metal::objc::answering_selector(receiver, concat!($name, "\0")) {
            Ok(selector) => {
                // SAFETY: `objc_msgSend` is entered with the registers and the
                // stack of a C call of the method it dispatches to; the
                // function type written here is that method's declaration.
                let method: unsafe extern "C-unwind" fn($crate::metal::objc::Id, $crate::metal::objc::Sel $(, $kind)*) -> $result =
                    unsafe { std::mem::transmute($crate::metal::objc::objc_msgSend as unsafe extern "C-unwind" fn()) };
                // SAFETY: the receiver is a live object that answers the
                // selector, both checked above; the arguments have the types
                // of the method's declaration.
                Ok::<$result, String>(unsafe { method(receiver, selector $(, $argument)*) })
            }
            Err(reason) => Err(reason),
        }
    }};
}

/// The selector of a name that ends with a NUL, once the receiver is known to
/// exist and to answer it.
pub fn answering_selector(receiver: Id, terminated_name: &str) -> Result<Sel, String> {
    let name = terminated_name.trim_end_matches('\0');
    if receiver.is_null() {
        return Err(format!("{name} has no receiver: the object it would be sent to is nil"));
    }
    let c_name = CStr::from_bytes_with_nul(terminated_name.as_bytes()).map_err(|_| format!("the selector {name} is not written as one C string"))?;
    // SAFETY: both names are NUL-terminated and the runtime keeps its own copies.
    let (selector, responds_to_selector) = unsafe { (sel_registerName(c_name.as_ptr()), sel_registerName(c"respondsToSelector:".as_ptr())) };
    // SAFETY: every object and every class answers `respondsToSelector:`,
    // declared `- (BOOL)respondsToSelector:(SEL)aSelector`.
    let responds: unsafe extern "C-unwind" fn(Id, Sel, Sel) -> u8 = unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C-unwind" fn()) };
    // SAFETY: the receiver is a live object and both selectors are registered.
    if unsafe { responds(receiver, responds_to_selector, selector) } != 0 { Ok(selector) } else { Err(format!("an object of class {} does not answer {name}", class_name(receiver))) }
}

/// Whether an object answers a selector named at run time; a nil object answers nothing.
pub fn answers(object: Id, name: &str) -> bool {
    answering_selector(object, &format!("{name}\0")).is_ok()
}

/// The value of a property named at run time: a getter without argument,
/// sent once the object is known to answer it. `T` is the getter's return
/// type, one for which every bit pattern is a value.
fn property<T: Copy>(object: Id, name: &str) -> Option<T> {
    let selector = answering_selector(object, &format!("{name}\0")).ok()?;
    // SAFETY: `objc_msgSend` is entered as the getter it dispatches to, which
    // takes no argument and returns a `T`.
    let getter: unsafe extern "C-unwind" fn(Id, Sel) -> T = unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C-unwind" fn()) };
    // SAFETY: the object is live and answers the selector, both checked above.
    Some(unsafe { getter(object, selector) })
}

pub fn class(name: &str) -> Result<Id, String> {
    let c_name = c_string(name);
    // SAFETY: the name is NUL-terminated; the runtime returns nil for a class it does not know.
    let class = unsafe { objc_getClass(c_name.as_ptr()) };
    if class.is_null() { Err(format!("the Objective-C class {name} does not exist")) } else { Ok(class) }
}

/// The name of an object's class, the evidence of which implementation stands behind it.
pub fn class_name(object: Id) -> String {
    if object.is_null() {
        return "nil".to_string();
    }
    // SAFETY: the object is live; the runtime returns a NUL-terminated name it owns.
    unsafe { CStr::from_ptr(object_getClassName(object)) }.to_string_lossy().into_owned()
}

/// An object a call returned, or the step that returned nil.
pub fn present(object: Id, step: &str) -> Result<Id, String> {
    if object.is_null() { Err(format!("{step} returned nil")) } else { Ok(object) }
}

/// One reference to an object, given back when the value is dropped.
pub struct Owned(Id);

impl Owned {
    /// Takes over the reference a `new`, `alloc`, `copy` or `Create` call returned.
    pub fn adopt(object: Id, step: &str) -> Result<Owned, String> {
        present(object, step).map(Owned)
    }

    /// Takes a reference of its own on an object the caller does not own.
    pub fn retain(object: Id, step: &str) -> Result<Owned, String> {
        send!(Id; present(object, step)?, "retain")?;
        Ok(Owned(object))
    }

    pub fn id(&self) -> Id {
        self.0
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        // An object that does not answer `release` cannot be given back, and
        // a destructor has nobody to tell.
        let _ = send!((); self.0, "release");
    }
}

/// `[[Class alloc] init]`, owned.
pub fn new_object(class_name: &str) -> Result<Owned, String> {
    let allocated = send!(Id; class(class_name)?, "alloc")?;
    Owned::adopt(send!(Id; allocated, "init")?, &format!("[[{class_name} alloc] init]"))
}

/// An autorelease pool that lasts as long as the value.
pub struct Pool(*mut c_void);

impl Pool {
    pub fn push() -> Pool {
        // SAFETY: the runtime returns a token that the matching pop takes.
        Pool(unsafe { objc_autoreleasePoolPush() })
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        // SAFETY: the token is the one this pool's push returned, popped once.
        unsafe { objc_autoreleasePoolPop(self.0) };
    }
}

/// `NSUTF8StringEncoding`.
const STRING_ENCODING_UTF8: usize = 4;

/// An `NSString` holding a copy of `text`.
pub fn string(text: &str) -> Result<Owned, String> {
    let allocated = send!(Id; class("NSString")?, "alloc")?;
    let string = send!(Id; allocated, "initWithBytes:length:encoding:", *const c_void: text.as_ptr().cast(), usize: text.len(), usize: STRING_ENCODING_UTF8)?;
    Owned::adopt(string, "[[NSString alloc] initWithBytes:length:encoding:]")
}

/// The text of an `NSString`; empty for nil.
pub fn text(string: Id) -> String {
    if string.is_null() {
        return String::new();
    }
    match send!(*const c_char; string, "UTF8String") {
        // SAFETY: the string returned a NUL-terminated buffer that lives as long as it does.
        Ok(characters) if !characters.is_null() => unsafe { CStr::from_ptr(characters) }.to_string_lossy().into_owned(),
        Ok(_) => String::new(),
        Err(reason) => format!("({reason})"),
    }
}

/// What an `NSError` says, for the text of an `Err`.
pub fn error_text(error: Id) -> String {
    if error.is_null() {
        return "no error object was returned".to_string();
    }
    match send!(Id; error, "localizedDescription") {
        Ok(description) => text(description),
        Err(reason) => reason,
    }
}

/// A BOOL property or query without argument, or nothing when the object does not have it.
pub fn flag(object: Id, name: &str) -> Option<bool> {
    property::<u8>(object, name).map(|answer| answer != 0)
}

/// An `NSUInteger` property, or nothing when the object does not have it.
pub fn unsigned(object: Id, name: &str) -> Option<usize> {
    property::<usize>(object, name)
}

pub fn yes_no(answer: bool) -> &'static str {
    if answer { "yes" } else { "no" }
}

/// Text on one line and of a bounded length, for a compiler's message quoted in a report.
pub fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(700).collect()
}

/// The runtime calls this with every exception before it unwinds.
extern "C" fn print_exception(exception: Id) -> Id {
    let name = send!(Id; exception, "name").map(text).unwrap_or_default();
    let reason = send!(Id; exception, "reason").map(text).unwrap_or_default();
    let _ = writeln!(std::io::stderr(), "an Objective-C exception was raised: {name}: {reason}");
    exception
}

/// Makes the runtime print every Objective-C exception as it is raised: an
/// exception that reaches Rust ends the process without a word otherwise.
/// The hook is looked up at run time, so that its absence costs the message
/// and nothing else.
pub fn print_exceptions() {
    let Ok(runtime) = Library::open(&["/usr/lib/libobjc.A.dylib"]) else { return };
    type Preprocessor = extern "C" fn(Id) -> Id;
    // SAFETY: the runtime declares `objc_setExceptionPreprocessor` as taking
    // and returning a function from an exception to an exception.
    let set_preprocessor: Result<unsafe extern "C" fn(Preprocessor) -> *mut c_void, String> = unsafe { runtime.function("objc_setExceptionPreprocessor") };
    if let Ok(set_preprocessor) = set_preprocessor {
        // SAFETY: the preprocessor is a function of the process, valid for as long as it runs.
        unsafe { set_preprocessor(print_exception) };
    }
}
