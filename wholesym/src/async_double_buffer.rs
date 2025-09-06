use std::ops::{Deref, DerefMut};
use tokio::sync::mpsc;

/// One side of a double buffer
pub struct BufferSide<T> {
    current_buffer: Option<T>,
    swap_sender: mpsc::Sender<T>,
    swap_receiver: mpsc::Receiver<T>,
}

impl<T> BufferSide<T> {
    /// Swap buffers with the other side
    /// Returns false if the other side has disconnected
    pub async fn swap(&mut self) -> bool
    where
        T: Send,
    {
        // Send our current buffer to the other side
        let buffer_to_send = self.current_buffer.take().unwrap();

        if self.swap_sender.send(buffer_to_send).await.is_err() {
            return false;
        }

        // Receive the other buffer
        match self.swap_receiver.recv().await {
            Some(buffer) => {
                self.current_buffer = Some(buffer);
                true
            }
            None => false,
        }
    }

    /// Swap buffers with the other side (blocking version)
    /// Returns false if the other side has disconnected
    ///
    /// This method can be used in `spawn_blocking` contexts where async is not available.
    pub fn swap_blocking(&mut self) -> bool
    where
        T: Send,
    {
        // Send our current buffer to the other side
        let buffer_to_send = self.current_buffer.take().unwrap();

        if self.swap_sender.blocking_send(buffer_to_send).is_err() {
            return false;
        }

        // Receive the other buffer
        match self.swap_receiver.blocking_recv() {
            Some(buffer) => {
                self.current_buffer = Some(buffer);
                true
            }
            None => false,
        }
    }
}

impl<T> Deref for BufferSide<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.current_buffer.as_ref().unwrap()
    }
}

impl<T> DerefMut for BufferSide<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.current_buffer.as_mut().unwrap()
    }
}

/// Create a double buffer from two buffer instances
pub fn double_buffer<T>(buffer_a: T, buffer_b: T) -> (BufferSide<T>, BufferSide<T>) {
    let (sender_a, receiver_b) = mpsc::channel(1);
    let (sender_b, receiver_a) = mpsc::channel(1);

    let side_a = BufferSide {
        current_buffer: Some(buffer_a),
        swap_sender: sender_a,
        swap_receiver: receiver_a,
    };

    let side_b = BufferSide {
        current_buffer: Some(buffer_b),
        swap_sender: sender_b,
        swap_receiver: receiver_b,
    };

    (side_a, side_b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_double_buffer_creation_and_access() {
        let (mut side_a, mut side_b) = double_buffer(vec![1, 2, 3], vec![4, 5, 6]);

        assert_eq!(*side_a, vec![1, 2, 3]);
        assert_eq!(*side_b, vec![4, 5, 6]);

        side_a.push(7);
        side_b.push(8);

        assert_eq!(*side_a, vec![1, 2, 3, 7]);
        assert_eq!(*side_b, vec![4, 5, 6, 8]);
    }

    #[tokio::test]
    async fn test_buffer_swap() {
        let (mut side_a, mut side_b) =
            double_buffer(String::from("buffer_a"), String::from("buffer_b"));

        assert_eq!(*side_a, "buffer_a");
        assert_eq!(*side_b, "buffer_b");

        let swap_a = tokio::spawn(async move {
            let success = side_a.swap().await;
            (side_a, success)
        });

        let swap_b = tokio::spawn(async move {
            let success = side_b.swap().await;
            (side_b, success)
        });

        let (side_a, success_a) = swap_a.await.unwrap();
        let (side_b, success_b) = swap_b.await.unwrap();

        assert!(success_a);
        assert!(success_b);
        assert_eq!(*side_a, "buffer_b");
        assert_eq!(*side_b, "buffer_a");
    }

    #[tokio::test]
    async fn test_multiple_swaps_deadlock_demo() {
        // This test demonstrates what would happen if we awaited serially - it would deadlock!
        // Uncomment the serial awaiting lines below to see the deadlock (test will timeout).

        let (side_a, side_b) = double_buffer(100, 200);

        let task_a = tokio::spawn(async move {
            let mut side = side_a;
            let mut log = Vec::new();

            for _ in 0..5 {
                log.push(*side);
                let success = side.swap().await;
                if !success {
                    panic!("Swap failed in task A");
                }
            }
            log.push(*side);
            log
        });

        let task_b = tokio::spawn(async move {
            let mut side = side_b;
            let mut log = Vec::new();

            for _ in 0..5 {
                log.push(*side);
                let success = side.swap().await;
                if !success {
                    panic!("Swap failed in task B");
                }
            }
            log.push(*side);
            log
        });

        // CORRECT: Use join to run both tasks in parallel
        let (log_a, log_b) = tokio::join!(task_a, task_b);

        // INCORRECT: Awaiting serially would cause deadlock
        // let log_a = task_a.await.unwrap();  // This would wait forever!
        // let log_b = task_b.await.unwrap();  // This would never be reached

        let log_a = log_a.unwrap();
        let log_b = log_b.unwrap();

        assert_eq!(log_a, vec![100, 200, 100, 200, 100, 200]);
        assert_eq!(log_b, vec![200, 100, 200, 100, 200, 100]);
    }

    #[tokio::test]
    async fn test_multiple_swaps() {
        let (side_a, side_b) = double_buffer(100, 200);

        let task_a = tokio::spawn(async move {
            let mut side = side_a;
            let mut log = Vec::new();

            for _ in 0..5 {
                log.push(*side);
                let success = side.swap().await;
                if !success {
                    panic!("Swap failed in task A");
                }
            }
            log.push(*side); // Final value after last swap
            log
        });

        let task_b = tokio::spawn(async move {
            let mut side = side_b;
            let mut log = Vec::new();

            for _ in 0..5 {
                log.push(*side);
                let success = side.swap().await;
                if !success {
                    panic!("Swap failed in task B");
                }
            }
            log.push(*side); // Final value after last swap
            log
        });

        let (log_a, log_b) = tokio::join!(task_a, task_b);
        let log_a = log_a.unwrap();
        let log_b = log_b.unwrap();

        // Verify that values alternated correctly
        // Task A should see: 100, 200, 100, 200, 100, 200
        // Task B should see: 200, 100, 200, 100, 200, 100
        assert_eq!(log_a, vec![100, 200, 100, 200, 100, 200]);
        assert_eq!(log_b, vec![200, 100, 200, 100, 200, 100]);
    }

    #[tokio::test]
    async fn test_modify_between_swaps() {
        let (mut side_a, mut side_b) = double_buffer(vec![1], vec![2]);

        side_a.push(10);
        side_b.push(20);

        assert_eq!(*side_a, vec![1, 10]);
        assert_eq!(*side_b, vec![2, 20]);

        let swap_a = tokio::spawn(async move {
            let success = side_a.swap().await;
            (side_a, success)
        });

        let swap_b = tokio::spawn(async move {
            let success = side_b.swap().await;
            (side_b, success)
        });

        let (mut side_a, _) = swap_a.await.unwrap();
        let (mut side_b, _) = swap_b.await.unwrap();

        assert_eq!(*side_a, vec![2, 20]);
        assert_eq!(*side_b, vec![1, 10]);

        side_a.push(30);
        side_b.push(40);

        assert_eq!(*side_a, vec![2, 20, 30]);
        assert_eq!(*side_b, vec![1, 10, 40]);
    }

    #[tokio::test]
    async fn test_swap_after_drop_returns_false() {
        let (mut side_a, side_b) = double_buffer(1, 2);

        drop(side_b);

        let success = side_a.swap().await;
        assert!(!success);
    }

    #[tokio::test]
    async fn test_custom_struct_buffer() {
        #[derive(Debug, PartialEq)]
        struct CustomData {
            value: i32,
            name: String,
        }

        let data_a = CustomData {
            value: 42,
            name: String::from("alpha"),
        };

        let data_b = CustomData {
            value: 84,
            name: String::from("beta"),
        };

        let (side_a, side_b) = double_buffer(data_a, data_b);

        assert_eq!(side_a.value, 42);
        assert_eq!(side_a.name, "alpha");
        assert_eq!(side_b.value, 84);
        assert_eq!(side_b.name, "beta");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_multiple_swaps_single_threaded() {
        // Test that swaps work correctly even on a single-threaded executor
        // This verifies that tokio's cooperative scheduling ensures progress
        let (side_a, side_b) = double_buffer(100, 200);

        let task_a = tokio::spawn(async move {
            let mut side = side_a;
            let mut log = Vec::new();

            for i in 0..5 {
                log.push(*side);
                println!("Task A: iteration {}, value: {}", i, *side);
                let success = side.swap().await;
                if !success {
                    panic!("Swap failed in task A at iteration {}", i);
                }
            }
            log.push(*side);
            println!("Task A: final value: {}", *side);
            log
        });

        let task_b = tokio::spawn(async move {
            let mut side = side_b;
            let mut log = Vec::new();

            for i in 0..5 {
                log.push(*side);
                println!("Task B: iteration {}, value: {}", i, *side);
                let success = side.swap().await;
                if !success {
                    panic!("Swap failed in task B at iteration {}", i);
                }
            }
            log.push(*side);
            println!("Task B: final value: {}", *side);
            log
        });

        // Even with join! on a single thread, both tasks will make progress
        // because awaiting on channels yields control back to the executor
        let (log_a, log_b) = tokio::join!(task_a, task_b);
        let log_a = log_a.unwrap();
        let log_b = log_b.unwrap();

        assert_eq!(log_a, vec![100, 200, 100, 200, 100, 200]);
        assert_eq!(log_b, vec![200, 100, 200, 100, 200, 100]);
    }

    #[tokio::test]
    async fn test_swap_cooperative_yielding() {
        // This test demonstrates that swap() is a yield point
        // Both tasks can make progress even without explicit yielding
        let (side_a, side_b) = double_buffer("A", "B");

        let task_a = tokio::spawn(async move {
            let mut side = side_a;
            let mut swaps = 0;

            // Tight loop with no explicit yields - only swap() yields
            loop {
                if !side.swap().await {
                    break;
                }
                swaps += 1;
                if swaps >= 100 {
                    break;
                }
            }
            swaps
        });

        let task_b = tokio::spawn(async move {
            let mut side = side_b;
            let mut swaps = 0;

            // Same tight loop on the other side
            loop {
                if !side.swap().await {
                    break;
                }
                swaps += 1;
                if swaps >= 100 {
                    break;
                }
            }
            swaps
        });

        let (swaps_a, swaps_b) = tokio::join!(task_a, task_b);

        // Both should complete 100 swaps successfully
        assert_eq!(swaps_a.unwrap(), 100);
        assert_eq!(swaps_b.unwrap(), 100);
    }

    #[tokio::test]
    async fn test_deref_and_deref_mut() {
        let (mut side_a, _side_b) = double_buffer(vec![1, 2, 3], vec![4, 5, 6]);

        let len = side_a.len();
        assert_eq!(len, 3);

        side_a.clear();
        assert_eq!(side_a.len(), 0);

        side_a.extend_from_slice(&[7, 8, 9]);
        assert_eq!(*side_a, vec![7, 8, 9]);
    }
}
