//! sqlx integration for [`EstimatorBand`].
//!
//! This module is only compiled when the `sqlx` feature is enabled. Keeping it
//! behind a feature avoids pulling sqlx into the WASM `takusu-worker` bundle.

use crate::EstimatorBand;
use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use sqlx::{Database, Decode, Encode, Type};

impl<DB: Database> Type<DB> for EstimatorBand
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

impl<'q, DB: Database> Encode<'q, DB> for EstimatorBand
where
    String: Encode<'q, DB> + Type<DB>,
{
    fn encode(self, buf: &mut <DB as Database>::ArgumentBuffer) -> Result<IsNull, BoxDynError> {
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

impl<'r, DB: Database> Decode<'r, DB> for EstimatorBand
where
    String: Decode<'r, DB>,
{
    fn decode(value: <DB as Database>::ValueRef<'r>) -> Result<Self, BoxDynError> {
        let s = <String as Decode<'r, DB>>::decode(value)?;
        s.parse().map_err(|e| BoxDynError::from(format!("{e}")))
    }
}
