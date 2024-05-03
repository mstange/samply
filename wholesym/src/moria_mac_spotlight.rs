// This code was taken from https://github.com/gimli-rs/moria/ , which is currently
// not released on crates.io.

use std::path::{Path, PathBuf};
use std::ptr;

use core_foundation::array::{CFArray, CFArrayRef};
use core_foundation::base::{CFType, CFTypeRef, TCFType, TCFTypeRef};
use core_foundation::impl_TCFType;
use core_foundation::string::CFString;
use core_foundation_sys::base::{
    kCFAllocatorDefault, CFAllocatorRef, CFIndex, CFOptionFlags, CFRelease, CFTypeID,
};
use core_foundation_sys::string::CFStringRef;
use libc::c_void;
use uuid::Uuid;

type Boolean = ::std::os::raw::c_uchar;
//const TRUE: Boolean = 1;
const FALSE: Boolean = 0;
#[repr(C)]
struct __MDQuery(c_void);
type MDQueryRef = *mut __MDQuery;
#[repr(C)]
struct __MDItem(c_void);
type MDItemRef = *mut __MDItem;

#[allow(non_upper_case_globals)]
const kMDQuerySynchronous: CFOptionFlags = 1;
#[link(name = "CoreServices", kind = "framework")]
extern "C" {
    #[link_name = "\u{1}_MDQueryCreate"]
    fn MDQueryCreate(
        allocator: CFAllocatorRef,
        queryString: CFStringRef,
        valueListAttrs: CFArrayRef,
        sortingAttrs: CFArrayRef,
    ) -> MDQueryRef;
    #[link_name = "\u{1}_MDQueryGetTypeID"]
    fn MDQueryGetTypeID() -> CFTypeID;
    #[link_name = "\u{1}_MDQueryExecute"]
    fn MDQueryExecute(query: MDQueryRef, optionFlags: CFOptionFlags) -> Boolean;
    #[link_name = "\u{1}_MDQueryGetResultCount"]
    fn MDQueryGetResultCount(query: MDQueryRef) -> CFIndex;
    #[link_name = "\u{1}_MDQueryGetResultAtIndex"]
    fn MDQueryGetResultAtIndex(query: MDQueryRef, idx: CFIndex) -> *const ::std::os::raw::c_void;
    #[link_name = "\u{1}_MDItemCreate"]
    fn MDItemCreate(allocator: CFAllocatorRef, path: CFStringRef) -> MDItemRef;
    #[link_name = "\u{1}_MDItemGetTypeID"]
    pub fn MDItemGetTypeID() -> CFTypeID;
    #[link_name = "\u{1}_MDItemCopyAttribute"]
    fn MDItemCopyAttribute(item: MDItemRef, name: CFStringRef) -> CFTypeRef;
    #[link_name = "\u{1}_kMDItemPath"]
    static mut kMDItemPath: CFStringRef;
}

struct MDQuery(MDQueryRef);

type Error = &'static str;

impl MDQuery {
    pub fn create(query_string: &str) -> Result<MDQuery, Error> {
        let cf_query_string = CFString::new(query_string);
        let query = unsafe {
            MDQueryCreate(
                kCFAllocatorDefault,
                ctref(&cf_query_string),
                ptr::null(),
                ptr::null(),
            )
        };
        if query.is_null() {
            return Err("MDQueryCreate failed");
        }
        unsafe { Ok(MDQuery::wrap_under_create_rule(query)) }
    }
    pub fn execute(&self) -> Result<CFIndex, Error> {
        if unsafe { MDQueryExecute(ctref(self), kMDQuerySynchronous) } == FALSE {
            return Err("MDQueryExecute failed");
        }
        unsafe { Ok(MDQueryGetResultCount(ctref(self))) }
    }
}
impl Drop for MDQuery {
    fn drop(&mut self) {
        unsafe { CFRelease(self.as_CFTypeRef()) }
    }
}
impl_TCFType!(MDQuery, MDQueryRef, MDQueryGetTypeID);

struct MDItem(MDItemRef);
impl Drop for MDItem {
    fn drop(&mut self) {
        unsafe { CFRelease(self.as_CFTypeRef()) }
    }
}
impl_TCFType!(MDItem, MDItemRef, MDItemGetTypeID);

#[inline]
fn ctref<T>(t: &T) -> T::Ref
where
    T: TCFType,
{
    t.as_concrete_TypeRef()
}

fn cast<T, U>(t: &T) -> Result<U, Error>
where
    T: TCFType,
    U: TCFType,
{
    if !t.instance_of::<U>() {
        return Err("dsym_paths attribute not an array");
    }

    let t: *const c_void = t.as_concrete_TypeRef().as_void_ptr();

    Ok(unsafe { U::wrap_under_get_rule(U::Ref::from_void_ptr(t)) })
}

/// Attempt to locate the Mach-O file inside a dSYM matching `uuid` using spotlight.
fn spotlight_locate_dsym_bundle(uuid: Uuid) -> Result<String, Error> {
    let uuid = uuid.hyphenated().to_string().to_uppercase();
    let query_string = format!("com_apple_xcode_dsym_uuids == {uuid}");
    let query = MDQuery::create(&query_string)?;
    let count = query.execute()?;
    for i in 0..count {
        let item = unsafe { MDQueryGetResultAtIndex(ctref(&query), i) as MDItemRef };
        let attr = unsafe { CFString::wrap_under_get_rule(kMDItemPath) };
        let cf_attr = unsafe { MDItemCopyAttribute(item, ctref(&attr)) };
        if cf_attr.is_null() {
            return Err("MDItemCopyAttribute failed");
        }
        let cf_attr = unsafe { CFType::wrap_under_get_rule(cf_attr) };
        if let Ok(path) = cast::<CFType, CFString>(&cf_attr) {
            return Ok(path.to_string());
        }
    }
    Err("dSYM not found")
}

/// Get the path to the Mach-O file containing DWARF debug info inside `bundle`.
fn spotlight_get_dsym_path(bundle: &str) -> Result<String, Error> {
    let cf_bundle_string = CFString::new(bundle);
    let bundle_item = unsafe { MDItemCreate(kCFAllocatorDefault, ctref(&cf_bundle_string)) };
    if bundle_item.is_null() {
        return Err("MDItemCreate failed");
    }
    let bundle_item = unsafe { MDItem::wrap_under_create_rule(bundle_item) };
    let attr = CFString::from_static_string("com_apple_xcode_dsym_paths");
    let cf_attr = unsafe {
        CFType::wrap_under_get_rule(MDItemCopyAttribute(ctref(&bundle_item), ctref(&attr)))
    };
    let cf_array = cast::<CFType, CFArray<CFType>>(&cf_attr)?;
    if let Some(cf_item) = cf_array.iter().next() {
        let cf_item = unsafe { CFType::wrap_under_get_rule(ctref(&*cf_item)) };
        return cast::<CFType, CFString>(&cf_item).map(|s| s.to_string());
    }
    Err("dsym_paths array is empty")
}

pub fn locate_dsym_using_spotlight(uuid: uuid::Uuid) -> Result<PathBuf, Error> {
    let bundle = spotlight_locate_dsym_bundle(uuid)?;
    Ok(Path::new(&bundle).join(spotlight_get_dsym_path(&bundle)?))
}
