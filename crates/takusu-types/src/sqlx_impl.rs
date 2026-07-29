//! sqlx integration for [`Quantity`] and [`Abandonability`].
//!
//! This module is only compiled when the `sqlx` feature is enabled, which
//! `takusu-storage` turns on. Keeping it behind a feature avoids pulling sqlx
//! into the WASM `takusu-worker` bundle.

use crate::time_types::{Date, TimeOfDay, Timestamp};
use crate::{Abandonability, Quantity, QuantityError};
use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use sqlx::{Database, Decode, Encode, Type};

impl<DB: Database> Type<DB> for Quantity
where
    i64: Type<DB>,
{
    fn type_info() -> <DB as Database>::TypeInfo {
        <i64 as Type<DB>>::type_info()
    }

    fn compatible(ty: &<DB as Database>::TypeInfo) -> bool {
        <i64 as Type<DB>>::compatible(ty)
    }
}

impl<'q, DB: Database> Encode<'q, DB> for Quantity
where
    i64: Encode<'q, DB> + Type<DB>,
{
    fn encode(self, buf: &mut <DB as Database>::ArgumentBuffer) -> Result<IsNull, BoxDynError> {
        <i64 as Encode<'q, DB>>::encode(self.get(), buf)
    }

    fn encode_by_ref(
        &self,
        buf: &mut <DB as Database>::ArgumentBuffer,
    ) -> Result<IsNull, BoxDynError> {
        <i64 as Encode<'q, DB>>::encode(self.get(), buf)
    }

    fn produces(&self) -> Option<<DB as Database>::TypeInfo> {
        Some(<i64 as Type<DB>>::type_info())
    }
}

impl<'r, DB: Database> Decode<'r, DB> for Quantity
where
    i64: Decode<'r, DB>,
{
    fn decode(value: <DB as Database>::ValueRef<'r>) -> Result<Self, BoxDynError> {
        let raw = <i64 as Decode<'r, DB>>::decode(value)?;
        Quantity::new(raw).map_err(|e: QuantityError| e.into())
    }
}

impl<DB: Database> Type<DB> for Abandonability
where
    f64: Type<DB>,
{
    fn type_info() -> <DB as Database>::TypeInfo {
        <f64 as Type<DB>>::type_info()
    }

    fn compatible(ty: &<DB as Database>::TypeInfo) -> bool {
        <f64 as Type<DB>>::compatible(ty)
    }
}

impl<'q, DB: Database> Encode<'q, DB> for Abandonability
where
    f64: Encode<'q, DB> + Type<DB>,
{
    fn encode(self, buf: &mut <DB as Database>::ArgumentBuffer) -> Result<IsNull, BoxDynError> {
        <f64 as Encode<'q, DB>>::encode(self.get(), buf)
    }

    fn encode_by_ref(
        &self,
        buf: &mut <DB as Database>::ArgumentBuffer,
    ) -> Result<IsNull, BoxDynError> {
        <f64 as Encode<'q, DB>>::encode(self.get(), buf)
    }

    fn produces(&self) -> Option<<DB as Database>::TypeInfo> {
        Some(<f64 as Type<DB>>::type_info())
    }
}

impl<'r, DB: Database> Decode<'r, DB> for Abandonability
where
    f64: Decode<'r, DB>,
{
    fn decode(value: <DB as Database>::ValueRef<'r>) -> Result<Self, BoxDynError> {
        let raw = <f64 as Decode<'r, DB>>::decode(value)?;
        Ok(Abandonability::new(raw))
    }
}

// ── TimeOfDay / Date / Timestamp (TEXT-backed) ─────────────────────────────
//
// All three are stored as TEXT in SQLite. We implement Type/Encode/Decode via
// String so that `sqlx::FromRow` derive and `.bind(&field)` work directly.

macro_rules! impl_sqlx_text {
    ($ty:ty) => {
        impl<DB: Database> Type<DB> for $ty
        where
            String: Type<DB>,
        {
            fn type_info() -> <DB as Database>::TypeInfo {
                <String as Type<DB>>::type_info()
            }

            fn compatible(ty: &<DB as Database>::TypeInfo) -> bool {
                <String as Type<DB>>::compatible(ty)
            }
        }

        impl<'q, DB: Database> Encode<'q, DB> for $ty
        where
            String: Encode<'q, DB> + Type<DB>,
        {
            fn encode(
                self,
                buf: &mut <DB as Database>::ArgumentBuffer,
            ) -> Result<IsNull, BoxDynError> {
                <String as Encode<'q, DB>>::encode(self.to_string(), buf)
            }

            fn encode_by_ref(
                &self,
                buf: &mut <DB as Database>::ArgumentBuffer,
            ) -> Result<IsNull, BoxDynError> {
                <String as Encode<'q, DB>>::encode(self.to_string(), buf)
            }

            fn produces(&self) -> Option<<DB as Database>::TypeInfo> {
                Some(<String as Type<DB>>::type_info())
            }
        }

        impl<'r, DB: Database> Decode<'r, DB> for $ty
        where
            String: Decode<'r, DB>,
        {
            fn decode(value: <DB as Database>::ValueRef<'r>) -> Result<Self, BoxDynError> {
                let s = <String as Decode<'r, DB>>::decode(value)?;
                s.parse().map_err(|e: crate::time_types::TimeParseError| {
                    BoxDynError::from(format!("{e}"))
                })
            }
        }
    };
}

impl_sqlx_text!(TimeOfDay);
impl_sqlx_text!(Date);
impl_sqlx_text!(Timestamp);
