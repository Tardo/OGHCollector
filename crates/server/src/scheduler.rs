use chrono::Local;
use tokio_schedule::{every, Job};

use crate::config::SERVER_CONFIG;

pub async fn start_scheduler() {
    let every_second = every(1)
        .seconds()
        .in_timezone(SERVER_CONFIG.get_timezone())
        .perform(|| async { println!("schedule_task event - {:?}", Local::now()) });
    every_second.await;
}