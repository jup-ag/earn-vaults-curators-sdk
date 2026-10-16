//! Fixed instruction fields retained for compatibility with the deployed IDL.
//! zeropod 0.4 supplies integers, but no longer supplies these fixed containers.
pub use zeropod::{Error as ZeroPodError, ZcElem, ZcValidate};
use {
    core::{mem::MaybeUninit, ops::Deref},
    wincode::{
        SchemaRead, SchemaWrite, TypeMeta,
        config::ConfigCore,
        io::{Reader, Writer},
    },
};

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PodOption<T: Copy> {
    tag: u8,
    value: T,
}

impl<T: ZcElem> PodOption<T> {
    pub fn none() -> Self {
        // SAFETY: ZcElem admits every bit pattern, including zero.
        Self {
            tag: 0,
            value: unsafe { core::mem::zeroed() },
        }
    }
    pub fn some(value: T) -> Self {
        Self { tag: 1, value }
    }
    pub fn get_ref(&self) -> Option<&T> {
        (self.tag == 1).then_some(&self.value)
    }
    pub fn get(&self) -> Option<T> {
        self.get_ref().copied()
    }
}
impl<T: ZcElem> ZcValidate for PodOption<T> {
    fn validate_ref(value: &Self) -> Result<(), ZeroPodError> {
        match value.tag {
            0 => Ok(()),
            1 => T::validate_ref(&value.value),
            _ => Err(ZeroPodError::InvalidTag),
        }
    }
}
// SAFETY: an initialized u8 and an align-1 ZcElem; every bit pattern is valid.
unsafe impl<T: ZcElem> ZcElem for PodOption<T> {}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PodString<const N: usize> {
    len: u8,
    data: [u8; N],
}
impl<const N: usize> TryFrom<&str> for PodString<N> {
    type Error = ZeroPodError;
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        if value.len() > N || value.len() > u8::MAX as usize {
            return Err(ZeroPodError::TooLong);
        }
        let mut result = Self {
            len: value.len() as u8,
            data: [0; N],
        };
        result.data[..value.len()].copy_from_slice(value.as_bytes());
        Ok(result)
    }
}
impl<const N: usize> PodString<N> {
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.data[..usize::from(self.len)]).expect("validated fixed string")
    }
}
impl<const N: usize> Deref for PodString<N> {
    type Target = str;
    fn deref(&self) -> &str {
        self.as_str()
    }
}
impl<const N: usize> ZcValidate for PodString<N> {
    fn validate_ref(value: &Self) -> Result<(), ZeroPodError> {
        let bytes = value
            .data
            .get(..usize::from(value.len))
            .ok_or(ZeroPodError::TooLong)?;
        core::str::from_utf8(bytes)
            .map(|_| ())
            .map_err(|_| ZeroPodError::InvalidUtf8)
    }
}
// SAFETY: only byte fields, no padding; string validity is checked before access.
unsafe impl<const N: usize> ZcElem for PodString<N> {}

macro_rules! codec {
    ([$($generic:tt)*] $ty:ty) => {
        // SAFETY: ZcElem guarantees a fixed, fully initialized byte representation.
        unsafe impl<$($generic)* C: ConfigCore> SchemaWrite<C> for $ty {
            type Src = Self;
            const TYPE_META: TypeMeta = TypeMeta::Static { size: size_of::<Self>(), zero_copy: true };
            fn size_of(_: &Self) -> wincode::WriteResult<usize> { Ok(size_of::<Self>()) }
            fn write(mut writer: impl Writer, value: &Self) -> wincode::WriteResult<()> {
                // SAFETY: Self is ZcElem, so every byte is initialized and there is no padding.
                writer.write(unsafe { core::slice::from_raw_parts(core::ptr::from_ref(value).cast(), size_of::<Self>()) })?;
                Ok(())
            }
        }
        // SAFETY: checks the full field and initializes the destination only on success.
        unsafe impl<'de, $($generic)* C: ConfigCore> SchemaRead<'de, C> for $ty {
            type Dst = Self;
            const TYPE_META: TypeMeta = TypeMeta::Static { size: size_of::<Self>(), zero_copy: true };
            fn read(mut reader: impl Reader<'de>, dst: &mut MaybeUninit<Self>) -> wincode::ReadResult<()> {
                let bytes = reader.take_scoped(size_of::<Self>())?;
                // SAFETY: the length is exact and Self is align-1 with every bit pattern valid.
                let value = unsafe { *bytes.as_ptr().cast::<Self>() };
                Self::validate_ref(&value).map_err(|_| wincode::ReadError::InvalidValue("invalid fixed field"))?;
                dst.write(value);
                Ok(())
            }
        }
        // SAFETY: Self is align-1 with no invalid Rust bit patterns or padding.
        unsafe impl<$($generic)* C: ConfigCore> wincode::config::ZeroCopy<C> for $ty {}
    };
}
codec!([T: ZcElem,] PodOption<T>);
codec!([const N: usize,] PodString<N>);
