//! Windows smart.
use super::*;

/// Queries physical disk number, drive model name, and SMART predict-failure status.
pub fn check_smart(device: &str) -> DiskHealth {
    let mut health = DiskHealth::default();
    let dev_w = wide(std::ffi::OsStr::new(device));
    let h_vol = unsafe {
        CreateFileW(
            dev_w.as_ptr(),
            0,
            SHARE_READ | SHARE_WRITE,
            std::ptr::null_mut(),
            OPEN_EXISTING,
            0,
            0,
        )
    };
    if h_vol == INVALID {
        health.status = SmartStatus::Unknown(format!("Could not open device {device}"));
        return health;
    }

    let mut extents: VOLUME_DISK_EXTENTS = unsafe { std::mem::zeroed() };
    let mut ret = 0u32;
    let ok = unsafe {
        DeviceIoControl(
            h_vol,
            IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS,
            std::ptr::null(),
            0,
            (&mut extents as *mut VOLUME_DISK_EXTENTS).cast(),
            std::mem::size_of::<VOLUME_DISK_EXTENTS>() as u32,
            &mut ret,
            std::ptr::null_mut(),
        )
    };
    unsafe { CloseHandle(h_vol) };

    if ok == 0 || extents.number_of_disk_extents == 0 {
        health.status = SmartStatus::Unknown("Could not map volume to physical drive".to_string());
        return health;
    }

    let disk_num = extents.extents[0].disk_number;
    health.disk_number = Some(disk_num);

    let phys_name = format!(r"\\.\PhysicalDrive{}", disk_num);
    let phys_w = wide(std::ffi::OsStr::new(&phys_name));
    let h_phys = unsafe {
        CreateFileW(
            phys_w.as_ptr(),
            0,
            SHARE_READ | SHARE_WRITE,
            std::ptr::null_mut(),
            OPEN_EXISTING,
            0,
            0,
        )
    };
    if h_phys == INVALID {
        health.status =
            SmartStatus::Unknown("Physical drive query requires administrator rights".to_string());
        return health;
    }

    // Query Device Model Name
    let query = STORAGE_PROPERTY_QUERY {
        property_id: 0,
        query_type: 0,
        additional_parameters: [0],
    };
    let mut desc_buf = [0u8; 1024];
    let mut desc_ret = 0u32;
    let desc_ok = unsafe {
        DeviceIoControl(
            h_phys,
            IOCTL_STORAGE_QUERY_PROPERTY,
            (&query as *const STORAGE_PROPERTY_QUERY).cast(),
            std::mem::size_of::<STORAGE_PROPERTY_QUERY>() as u32,
            desc_buf.as_mut_ptr().cast(),
            desc_buf.len() as u32,
            &mut desc_ret,
            std::ptr::null_mut(),
        )
    };
    if desc_ok != 0 {
        let product_id_offset = u32::from_le_bytes(desc_buf[16..20].try_into().unwrap()) as usize;
        if product_id_offset > 0 && product_id_offset < desc_buf.len() {
            let s = &desc_buf[product_id_offset..];
            let len = s.iter().position(|&b| b == 0).unwrap_or(s.len());
            let name = String::from_utf8_lossy(&s[..len]).trim().to_string();
            if !name.is_empty() {
                health.model = Some(name);
            }
        }
    }

    // Query SMART Predict Failure
    let mut pred: STORAGE_PREDICT_FAILURE = unsafe { std::mem::zeroed() };
    let mut pred_ret = 0u32;
    let pred_ok = unsafe {
        DeviceIoControl(
            h_phys,
            IOCTL_STORAGE_PREDICT_FAILURE,
            std::ptr::null(),
            0,
            (&mut pred as *mut STORAGE_PREDICT_FAILURE).cast(),
            std::mem::size_of::<STORAGE_PREDICT_FAILURE>() as u32,
            &mut pred_ret,
            std::ptr::null_mut(),
        )
    };

    if pred_ok != 0 {
        if pred.predict_failure != 0 {
            health.status = SmartStatus::Failing(
                "Drive controller reports imminent hardware failure".to_string(),
            );
        } else {
            health.status = SmartStatus::Healthy;
        }
    } else {
        // Fallback: Query NVMe SMART / Health Log Page (Log ID 2) via IOCTL_STORAGE_QUERY_PROPERTY
        #[repr(C)]
        struct NVME_QUERY {
            property_id: u32,
            query_type: u32,
            protocol_type: u32,
            data_type: u32,
            request_value: u32,
            request_sub_value: u32,
            offset: u32,
            length: u32,
            fixed_return: u32,
            reserved: [u32; 3],
        }
        let nvme_q = NVME_QUERY {
            property_id: 50, // StorageDeviceProtocolSpecificProperty
            query_type: 0,
            protocol_type: 3, // ProtocolTypeNvme
            data_type: 2,     // NVMeDataTypeLogPage
            request_value: 2, // SMART / Health Information
            request_sub_value: 0,
            offset: std::mem::size_of::<NVME_QUERY>() as u32,
            length: 512,
            fixed_return: 0,
            reserved: [0; 3],
        };
        let mut nvme_buf = [0u8; 1024];
        let mut nvme_ret = 0u32;
        let nvme_ok = unsafe {
            DeviceIoControl(
                h_phys,
                IOCTL_STORAGE_QUERY_PROPERTY,
                (&nvme_q as *const NVME_QUERY).cast(),
                std::mem::size_of::<NVME_QUERY>() as u32,
                nvme_buf.as_mut_ptr().cast(),
                nvme_buf.len() as u32,
                &mut nvme_ret,
                std::ptr::null_mut(),
            )
        };
        if nvme_ok != 0 && nvme_ret >= 48 {
            let offset = u32::from_le_bytes(nvme_buf[24..28].try_into().unwrap_or([0; 4])) as usize;
            if offset > 0 && offset < nvme_buf.len() {
                let critical_warning = nvme_buf[offset];
                if critical_warning != 0 {
                    health.status = SmartStatus::Failing(format!(
                        "NVMe controller reports critical warning flag (0x{:02X})",
                        critical_warning
                    ));
                } else {
                    health.status = SmartStatus::Healthy;
                }
            } else {
                health.status = SmartStatus::Unknown(
                    "SMART predict failure query not supported on this interface".to_string(),
                );
            }
        } else {
            health.status = SmartStatus::Unknown(
                "SMART predict failure query not supported on this interface".to_string(),
            );
        }
    }
    unsafe { CloseHandle(h_phys) };

    health
}
