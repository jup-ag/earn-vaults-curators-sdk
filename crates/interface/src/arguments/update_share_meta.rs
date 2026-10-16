use {
    crate::encoding,
    codama_macros::CodamaType,
    encoding::{PodOption, PodString, ZcValidate},
    wincode::{SchemaRead, SchemaWrite},
};

#[derive(SchemaRead, SchemaWrite, CodamaType)]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct UpdateShareMetaArgs {
    #[codama(type = option(struct(field("len", number(u8)), field("value", fixed_size(string, 25))), fixed))]
    pub metadata_name: PodOption<PodString<25>>,
    #[codama(type = option(struct(field("len", number(u8)), field("value", fixed_size(string, 5))), fixed))]
    pub metadata_symbol: PodOption<PodString<5>>,
    #[codama(type = option(struct(field("len", number(u8)), field("value", fixed_size(string, 200))), fixed))]
    pub metadata_uri: PodOption<PodString<200>>,
}

impl ZcValidate for UpdateShareMetaArgs {
    fn validate_ref(value: &Self) -> Result<(), zeropod::Error> {
        ZcValidate::validate_ref(&value.metadata_name)?;
        ZcValidate::validate_ref(&value.metadata_symbol)?;
        ZcValidate::validate_ref(&value.metadata_uri)?;
        Ok(())
    }
}
