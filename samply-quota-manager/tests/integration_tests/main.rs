use std::fs;
use std::time::{Duration, SystemTime};

use samply_quota_manager::QuotaManager;
use tempfile::TempDir;

#[tokio::test]
async fn test_quota_manager_size_limit() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("quota.db");
    let quota_dir = temp_dir.path().join("quota");
    fs::create_dir(&quota_dir).unwrap();

    // Create quota manager with 1000 byte size limit.
    let quota_manager = QuotaManager::new(&quota_dir, &db_path).unwrap();
    quota_manager.set_max_total_size(Some(1000));
    let notifier = quota_manager.notifier();

    // Create three files, each 400 bytes big.
    let a_400 = quota_dir.join("a_400.txt");
    let b_400 = quota_dir.join("b_400.txt");
    let c_400 = quota_dir.join("c_400.txt");

    fs::write(&a_400, vec![0u8; 400]).unwrap();
    fs::write(&b_400, vec![0u8; 400]).unwrap();
    fs::write(&c_400, vec![0u8; 400]).unwrap();

    let now = SystemTime::now();
    notifier.on_file_created(&a_400, 400, now);
    notifier.on_file_created(&b_400, 400, now);
    notifier.on_file_created(&c_400, 400, now);

    // Trigger eviction.
    notifier.trigger_eviction_if_needed();

    // Wait for eviction to complete.
    quota_manager.finish().await;

    // Check that a_400 was deleted - that's the oldest file.
    assert!(!a_400.exists());
    assert!(b_400.exists());
    assert!(c_400.exists());

    // Check that the new size is 800 bytes.
    let quota_manager = QuotaManager::new(&quota_dir, &db_path).unwrap();
    assert_eq!(quota_manager.current_total_size(), 800);
    quota_manager.finish().await;
}

#[tokio::test]
async fn test_quota_manager_age_limit() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("quota.db");
    let quota_dir = temp_dir.path().join("quota");
    fs::create_dir(&quota_dir).unwrap();

    // Create quota manager with 5 second age limit.
    let quota_manager = QuotaManager::new(&quota_dir, &db_path).unwrap();
    quota_manager.set_max_age(Some(5));
    let notifier = quota_manager.notifier();

    // Create two files.
    let a_100 = quota_dir.join("a_100.txt");
    let b_100 = quota_dir.join("b_100.txt");

    fs::write(&a_100, vec![0u8; 100]).unwrap();
    fs::write(&b_100, vec![0u8; 100]).unwrap();

    let old_time = SystemTime::now() - Duration::from_secs(10);

    notifier.on_file_created(&b_100, 100, old_time);
    notifier.on_file_created(&a_100, 100, old_time);

    // Access b_100 to make it more recent. We pick a time in the
    // future in case this test is slow to execute; the QuotaManager
    // calls `SystemTime::now()` when performing the eviction, so we
    // don't really have a fully controlled synthetic timeline, and
    // this test might be a bit brittle as a result.
    let new_time = SystemTime::now() + Duration::from_secs(100);
    notifier.on_file_accessed(&b_100, new_time);

    // Trigger eviction.
    notifier.trigger_eviction_if_needed();

    // Wait for eviction to complete.
    quota_manager.finish().await;

    // Check that only the newer file remains.
    assert!(!a_100.exists());
    assert!(b_100.exists());

    // Check that the new size is 100 bytes.
    let quota_manager = QuotaManager::new(&quota_dir, &db_path).unwrap();
    assert_eq!(quota_manager.current_total_size(), 100);
    quota_manager.finish().await;
}

#[tokio::test]
async fn test_quota_manager_size_limit_complex() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("quota.db");
    let quota_dir = temp_dir.path().join("quota");
    fs::create_dir(&quota_dir).unwrap();

    // Create quota manager with a 1600 byte size limit.
    let quota_manager = QuotaManager::new(&quota_dir, &db_path).unwrap();
    let notifier = quota_manager.notifier();

    let ref_time = SystemTime::now() - Duration::from_secs(100);

    // Create various files with different sizes.
    let make_file = |size, name| {
        let filename = quota_dir.join(name);
        fs::write(&filename, vec![0u8; size]).unwrap();
        notifier.on_file_created(&filename, size as u64, ref_time);
        filename
    };

    let b_20 = make_file(20, "b_20.txt");
    let c_100 = make_file(100, "c_100.txt");
    let i_1000 = make_file(1000, "i_1000.txt");
    let d_160 = make_file(160, "d_160.txt");
    let j_50 = make_file(50, "j_50.txt");
    let h_200 = make_file(200, "h_200.txt");
    let g_150 = make_file(150, "g_150.txt");
    let a_60 = make_file(60, "a_60.txt");
    let e_80 = make_file(80, "e_80.txt");
    let f_800 = make_file(800, "f_800.txt");

    notifier.on_file_accessed(&a_60, ref_time + Duration::from_secs(10));
    notifier.on_file_accessed(&b_20, ref_time + Duration::from_secs(20));
    notifier.on_file_accessed(&c_100, ref_time + Duration::from_secs(30));
    notifier.on_file_accessed(&d_160, ref_time + Duration::from_secs(40));
    notifier.on_file_accessed(&e_80, ref_time + Duration::from_secs(50));
    notifier.on_file_accessed(&f_800, ref_time + Duration::from_secs(60));
    notifier.on_file_accessed(&g_150, ref_time + Duration::from_secs(70));
    notifier.on_file_accessed(&h_200, ref_time + Duration::from_secs(80));
    notifier.on_file_accessed(&i_1000, ref_time + Duration::from_secs(90));
    notifier.on_file_accessed(&j_50, ref_time + Duration::from_secs(100));

    // Enforce a limit of 1600 bytes.
    assert_eq!(quota_manager.current_total_size(), 2620);
    quota_manager.set_max_total_size(Some(1600));
    notifier.trigger_eviction_if_needed();
    quota_manager.finish().await;

    // Check that only the most-recently accessed files survived,
    // as many as fit.
    assert!(j_50.exists()); // 50
    assert!(i_1000.exists()); // 1050
    assert!(h_200.exists()); // 1250
    assert!(g_150.exists()); // 1400
    assert!(!f_800.exists()); // 1400 + 800 = 2200 > 1600, delete
    assert!(e_80.exists()); // 1480
    assert!(!d_160.exists()); // 1480 + 160 = 1640 > 1600, delete
    assert!(c_100.exists()); // 1580
    assert!(b_20.exists()); // 1600
    assert!(!a_60.exists()); // 1600 + 60 = 1660 > 1600, delete
}

#[tokio::test]
async fn test_quota_manager_empty_dirs_cleaned() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("quota.db");
    let quota_dir = temp_dir.path().join("quota");
    fs::create_dir(&quota_dir).unwrap();

    let quota_manager = QuotaManager::new(&quota_dir, &db_path).unwrap();
    let notifier = quota_manager.notifier();

    let ref_time = SystemTime::now() - Duration::from_secs(100);

    let make_file = |size, name| {
        let filename = quota_dir.join(name);
        fs::write(&filename, vec![0u8; size]).unwrap();
        notifier.on_file_created(&filename, size as u64, ref_time);
        filename
    };

    fs::create_dir_all(quota_dir.join("dir1/dir2/dir3")).unwrap();
    let a_60 = make_file(60, "dir1/dir2/dir3/a_60.txt");
    assert!(quota_dir.join("dir1").exists());

    // Delete the file by enforcig an age limit of 1 second.
    assert_eq!(quota_manager.current_total_size(), 60);
    quota_manager.set_max_age(Some(1));
    notifier.trigger_eviction_if_needed();
    quota_manager.finish().await;

    // Check that the quota_dir survived, but nothing inside of it.
    assert!(quota_dir.exists());
    assert!(!quota_dir.join("dir1").exists());
    assert!(!quota_dir.join("dir1/dir2").exists());
    assert!(!quota_dir.join("dir1/dir2/dir3").exists());
    assert!(!a_60.exists());
}

#[tokio::test]
async fn test_quota_manager_nonempty_dirs_remain() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("quota.db");
    let quota_dir = temp_dir.path().join("quota");
    fs::create_dir(&quota_dir).unwrap();

    let quota_manager = QuotaManager::new(&quota_dir, &db_path).unwrap();
    let notifier = quota_manager.notifier();

    let ref_time = SystemTime::now() - Duration::from_secs(100);

    let make_file = |size, name| {
        let filename = quota_dir.join(name);
        fs::write(&filename, vec![0u8; size]).unwrap();
        notifier.on_file_created(&filename, size as u64, ref_time);
        filename
    };

    fs::create_dir_all(quota_dir.join("dir1/dir2/dir3")).unwrap();
    let a_60 = make_file(60, "dir1/dir2/dir3/a_60.txt");
    let b_40 = make_file(40, "dir1/b_40.txt");
    assert!(quota_dir.join("dir1").exists());
    notifier.on_file_accessed(&b_40, ref_time + Duration::from_secs(20));

    // Delete a_60 by enforcig a size limit of 50 bytes.
    assert_eq!(quota_manager.current_total_size(), 100);
    quota_manager.set_max_total_size(Some(50));
    notifier.trigger_eviction_if_needed();
    quota_manager.finish().await;

    // Check that a_60 and dir2 + dir3 have been deleted. dir1 and b_40.txt need to have survived.
    assert!(!quota_dir.join("dir1/dir2").exists());
    assert!(!quota_dir.join("dir1/dir2/dir3").exists());
    assert!(!a_60.exists());
    assert!(quota_dir.exists());
    assert!(quota_dir.join("dir1").exists());
    assert!(b_40.exists());
}
