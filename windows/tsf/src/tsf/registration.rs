use super::*;
use windows::Win32::UI::TextServices::TF_LANGUAGEPROFILE;

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

/// Removes the TSF registration and this view's CLSID key. The 64- and 32-bit DLLs share the TSF
/// part (profile, categories) but each has its own CLSID key, so whichever is unregistered second
/// finds the TSF part gone: that is success as long as the profile is really no longer listed.
pub(super) fn unregister() -> Result<()> {
    let mut failure = None;
    com_scope(|| {
        let mut note = |result: Result<()>| {
            if let Err(error) = absent_is_ok(result) {
                failure.get_or_insert(error);
            }
        };
        let categories: ITfCategoryMgr =
            unsafe { CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)? };
        let _ = unsafe {
            categories.UnregisterCategory(&CLSID, &GUID_TFCAT_TIPCAP_IMMERSIVESUPPORT, &CLSID)
        };
        for category in CATEGORIES {
            note(unsafe { categories.UnregisterCategory(&CLSID, &category, &CLSID) });
        }
        let profiles: ITfInputProcessorProfiles = unsafe {
            CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)?
        };
        note(unsafe { profiles.RemoveLanguageProfile(&CLSID, LANG_ZH_CN, &PROFILE) });
        note(unsafe { profiles.Unregister(&CLSID) });
        if failure.is_some() && !profile_listed(&profiles) {
            failure = None;
        }
        Ok(())
    })?;
    absent_is_ok(CLASSES_ROOT.remove_tree(format!("CLSID\\{CLSID_TEXT}")))?;
    failure.map_or(Ok(()), Err)
}

fn profile_listed(profiles: &ITfInputProcessorProfiles) -> bool {
    let Ok(list) = (unsafe { profiles.EnumLanguageProfiles(LANG_ZH_CN) }) else {
        return true;
    };
    let mut item = [TF_LANGUAGEPROFILE::default()];
    loop {
        let mut fetched = 0;
        if unsafe { list.Next(&mut item, &mut fetched) }.is_err() || fetched == 0 {
            return false;
        }
        if item[0].clsid == CLSID && item[0].guidProfile == PROFILE {
            return true;
        }
    }
}
