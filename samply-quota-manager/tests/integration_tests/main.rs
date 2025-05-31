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

