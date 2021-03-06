#![no_std]
#![feature(allocator_api)]
#![feature(ptr_internals)]
#![feature(try_reserve)]
#![feature(dropck_eyepatch)]
#![feature(rustc_private)]

extern crate alloc;
use alloc::alloc::handle_alloc_error;
use alloc::alloc::{Global, Layout};
use alloc::boxed::Box;
use core::alloc::Alloc;
use core::mem;
use core::ptr::{NonNull, Unique};

pub struct RealBox<T, A: Alloc = Global> {
    ptr: Unique<T>,
    a: A,
}

impl<T, A: Alloc> RealBox<T, A> {
    /// Gets a raw pointer to the start of the allocation. Note that this is
    /// Unique::empty() if `cap = 0` or T is zero-sized. In the former case, you must
    /// be careful.
    pub fn ptr(&self) -> *mut T {
        self.ptr.as_ptr()
    }

    /// Returns a shared reference to the allocator backing this RawVec.
    pub fn alloc(&self) -> &A {
        &self.a
    }

    /// Returns a mutable reference to the allocator backing this RawVec.
    pub fn alloc_mut(&mut self) -> &mut A {
        &mut self.a
    }

    fn current_layout(&self) -> Option<Layout> {
        unsafe {
            let align = mem::align_of::<T>();
            let size = mem::size_of::<T>();
            Some(Layout::from_size_align_unchecked(size, align))
        }
    }
}

impl<T, A: Alloc> RealBox<T, A> {
    pub unsafe fn dealloc_buffer(&mut self) {
        let elem_size = mem::size_of::<T>();
        if elem_size != 0 {
            if let Some(layout) = self.current_layout() {
                self.a.dealloc(NonNull::from(self.ptr).cast(), layout);
            }
        }
    }
}

unsafe impl<#[may_dangle] T, A: Alloc> Drop for RealBox<T, A> {
    fn drop(&mut self) {
        unsafe {
            self.dealloc_buffer();
        }
    }
}

impl<T, A: Alloc> RealBox<T, A> {
    pub(crate) fn new_in(a: A) -> Self {
        RealBox::allocate_in(true, a)
    }

    fn allocate_in(zeroed: bool, mut a: A) -> Self {
        let elem_size = mem::size_of::<T>();

        // handles ZSTs and `cap = 0` alike
        let ptr = if elem_size == 0 {
            NonNull::<T>::dangling()
        } else {
            let align = mem::align_of::<T>();
            let layout = Layout::from_size_align(elem_size, align).unwrap();
            let result = if zeroed {
                unsafe { a.alloc_zeroed(layout) }
            } else {
                unsafe { a.alloc(layout) }
            };
            match result {
                Ok(ptr) => ptr.cast(),
                Err(_) => handle_alloc_error(layout),
            }
        };

        RealBox { ptr: ptr.into(), a }
    }
}

impl<T> RealBox<T, Global> {
    pub fn new() -> Self {
        Self::new_in(Global)
    }

    /// Converts the entire buffer into `Box<T>`.
    pub unsafe fn into_box(self) -> Box<T> {
        let output: Box<T> = Box::from_raw(self.ptr());
        mem::forget(self);
        output
    }
}

impl<T> RealBox<T, Global> {
    pub fn heap_init<F>(initialize: F) -> Box<T>
    where
        F: Fn(&mut T),
    {
        unsafe {
            let mut t = Self::new_in(Global).into_box();
            initialize(t.as_mut());
            t
        }
    }
}

impl<T, A: Alloc> RealBox<T, A> {
    pub fn new_with_allocator(a: A) -> Self {
        Self::new_in(a)
    }
}

impl<T, A: Alloc> RealBox<T, A> {
    pub unsafe fn from_raw_parts(ptr: *mut T, a: A) -> Self {
        RealBox {
            ptr: Unique::new_unchecked(ptr),
            a,
        }
    }
}

impl<T> RealBox<T, Global> {
    pub fn from_box(mut slice: Box<[T]>) -> Self {
        unsafe {
            let result = RealBox::from_raw_parts(slice.as_mut_ptr(), Global);
            mem::forget(slice);
            result
        }
    }
}

#[cfg(test)]
mod test {
    use crate::*;

    #[test]
    fn test_naive_i32() {
        let t = RealBox::<i32>::new();
        assert_ne!(t.ptr.as_ptr(), core::ptr::null_mut());
    }

    extern crate std;
    use std::alloc::System;

    #[test]
    fn test_alloc_with_system() {
        let t = RealBox::<i32, System>::new_with_allocator(System);
        assert_ne!(t.ptr.as_ptr(), core::ptr::null_mut());
    }

    //#[test]
    //#[should_panic] // This should OOM and cargo test cannot unwind it!
    //fn test_big() {
    //    use std::boxed::Box;
    //    let _ = Box::new([[0;1000];1000]);
    //}

    #[test]
    fn test_pure_big() {
        let t = RealBox::<[[i32; 100]; 1000]>::new();
        assert_ne!(t.ptr.as_ptr(), core::ptr::null_mut());
    }

    struct DummyStruct;
    #[test]
    fn test_zero() {
        use core::ptr::NonNull;
        let t = RealBox::<DummyStruct>::new();
        assert_eq!(t.ptr.as_ptr(), NonNull::<_>::dangling().as_ptr());
    }

    #[test]
    fn test_drop() {
        let t = RealBox::<[[i32; 10000]; 1000]>::new();
        let ptr = t.ptr.as_ptr();
        drop(t);
        let t = RealBox::<[[i32; 10000]; 1000]>::new();
        assert_eq!(ptr, t.ptr.as_ptr());
    }

    #[test]
    fn test_heap_init() {
        extern crate libc;
        use core::ffi::c_void;

        #[derive(Debug)]
        struct Obj {
            x: u32,
            y: f64,
            a: [u8; 4],
        }

        let stack_obj = Obj {
            x: 12,
            y: 0.9,
            a: [0xff, 0xfe, 0xfd, 0xfc],
        };

        let heap_obj = RealBox::<Obj>::heap_init(|mut t| {
            t.x = 12;
            t.y = 0.9;
            t.a = [0xff, 0xfe, 0xfd, 0xfc]
        });

        let size = mem::size_of::<Obj>();

        unsafe {
            assert_eq!(
                libc::memcmp(
                    &stack_obj as *const Obj as *const c_void,
                    Box::into_raw(heap_obj) as *const c_void,
                    size
                ),
                0
            );
        }
    }
}
