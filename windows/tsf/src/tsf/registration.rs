use super::*;

pub(super) fn register() -> Result<()> {
    let key = CLASSES_ROOT.create(format!("CLSID\\{CLSID_TEXT}"))?;
    key.set_string("", DESCRIPTION)?;
    let inproc = key.create("InprocServer32")?;
    inproc.set_string("", module_path()?)?;
    inproc.set_string("ThreadingModel", "Apartment")?;
    com_scope(|| {
        let profiles: ITfInputProcessorProfiles = unsafe {
            CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)?
        };
        let description = wide(DESCRIPTION);
        let icon = wide(&module_path()?);
        unsafe {
            profiles.Register(&CLSID)?;
            profiles.AddLanguageProfile(&CLSID, LANG_ZH_CN, &PROFILE, &description, &icon, 0)?;
            let _ = profiles.EnableLanguageProfileByDefault(&CLSID, LANG_ZH_CN, &PROFILE, true);
        }
        let categories: ITfCategoryMgr =
            unsafe { CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)? };
        for category in CATEGORIES {
            unsafe {
                categories.RegisterCategory(&CLSID, &category, &CLSID)?;
            }
        }
        Ok(())
    })
}

pub(super) fn unregister() -> Result<()> {
    com_scope(|| {
        let categories: ITfCategoryMgr =
            unsafe { CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)? };
        let _ = unsafe {
            categories.UnregisterCategory(&CLSID, &GUID_TFCAT_TIPCAP_IMMERSIVESUPPORT, &CLSID)
        };
        for category in CATEGORIES {
            absent_is_ok(unsafe { categories.UnregisterCategory(&CLSID, &category, &CLSID) })?;
        }
        let profiles: ITfInputProcessorProfiles = unsafe {
            CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)?
        };
        absent_is_ok(unsafe { profiles.RemoveLanguageProfile(&CLSID, LANG_ZH_CN, &PROFILE) })?;
        absent_is_ok(unsafe { profiles.Unregister(&CLSID) })?;
        Ok(())
    })?;
    absent_is_ok(CLASSES_ROOT.remove_tree(format!("CLSID\\{CLSID_TEXT}")))?;
    Ok(())
}
