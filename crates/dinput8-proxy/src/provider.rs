//! Process-resident DirectInput provider. Registration does not call Core or
//! claim plugin readiness. Every real caller is forwarded once with its ABI.
use std::{ffi::c_void, sync::{Mutex,atomic::{AtomicUsize,Ordering}}};
pub type Create = unsafe extern "system" fn(*mut c_void,u32,*const c_void,*mut *mut c_void,*mut c_void)->i32;
pub struct Provider { module:Mutex<usize>, target:AtomicUsize }
impl Provider {
    pub const fn new()->Self {Self{module:Mutex::new(0),target:AtomicUsize::new(0)}}
    /// Called only after the runtime has admitted/pinned exact Core bytes and
    /// its private namespace. This validates an internal module, not caller trust.
    pub unsafe fn register(&self,module:*mut c_void,proxy:*mut c_void)->i32 {
        if module.is_null()||proxy.is_null()||module==proxy{return 0;}
        let Ok(mut current)=self.module.lock()else{return 0;};
        if *current!=0{return if *current==module as usize{2}else{0};}
        let entry=unsafe{GetProcAddress(module,c"DirectInput8Create".as_ptr().cast())};
        if entry.is_null(){return 0;}
        // Confirm the export belongs to this module; reject forwarded exports.
        let mut owner=std::ptr::null_mut();
        if unsafe{GetModuleHandleExW(6,entry.cast(),&mut owner)}==0||owner!=module{return 0;}
        // Retain both successful and reachable callbacks for the process lifetime.
        let mut pinned=std::ptr::null_mut();
        if unsafe{GetModuleHandleExW(5,entry.cast(),&mut pinned)}==0||pinned!=module{return 0;}
        *current=module as usize;self.target.store(entry as usize,Ordering::Release);1
    }
    pub unsafe fn forward(&self,system:usize,instance:*mut c_void,version:u32,iid:*const c_void,out:*mut *mut c_void,outer:*mut c_void)->i32 {
        let selected=self.target.load(Ordering::Acquire);let target=if selected==0{system}else{selected};
        if target==0{return 0x80004005u32 as i32;}
        let call:Create=unsafe{std::mem::transmute(target)};
        unsafe{call(instance,version,iid,out,outer)}
    }
}
#[link(name="kernel32")] unsafe extern "system" {
    fn GetProcAddress(module:*mut c_void,name:*const u8)->*mut c_void;
    fn GetModuleHandleExW(flags:u32,name:*const u16,module:*mut *mut c_void)->i32;
}
