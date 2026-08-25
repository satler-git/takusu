package expo.modules.takususerver

import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.locks.ReentrantLock

/**
 * Coordinates exclusive access to the microphone between the ambient agent
 * service and the in-app recorder. The lock is used to protect a simple
 * in-use flag so that [release] can be called from any thread (for example,
 * a recording thread's finally block or a service teardown path).
 */
object MicrophoneCoordinator {
    private val lock = ReentrantLock()
    private val available = lock.newCondition()
    private val inUse = AtomicBoolean(false)

    fun tryAcquire(timeoutMs: Long = 1000): Boolean {
        lock.lock()
        try {
            val deadline = System.nanoTime() + TimeUnit.MILLISECONDS.toNanos(timeoutMs)
            while (inUse.get()) {
                val remaining = deadline - System.nanoTime()
                if (remaining <= 0) {
                    return false
                }
                available.await(remaining, TimeUnit.NANOSECONDS)
            }
            inUse.set(true)
            return true
        } catch (e: InterruptedException) {
            Thread.currentThread().interrupt()
            return false
        } finally {
            lock.unlock()
        }
    }

    fun release() {
        lock.lock()
        try {
            if (inUse.compareAndSet(true, false)) {
                available.signal()
            }
        } finally {
            lock.unlock()
        }
    }
}
