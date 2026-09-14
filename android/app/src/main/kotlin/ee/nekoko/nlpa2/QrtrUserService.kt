package ee.nekoko.nlpa2

import android.util.Log
import kotlin.system.exitProcess

/**
 * The privileged half of the QRTR reader: the socket on the modem's bus.
 *
 * Shizuku starts this class in a process of its own under the `shell` user,
 * which is the only user allowed to open such a socket; the app may not open
 * one and may not use one this process opened either. So the socket stays
 * here, and the app asks for what a transport needs from it — where it is
 * addressed, one datagram out, one datagram in — and drives the modem from the
 * answers.
 *
 * The Shizuku server instantiates this class reflectively, so it has to stay
 * public and keep a constructor without arguments.
 */
class QrtrUserService : IQrtrService.Stub() {
    /** Descriptors handed out, so stopping the service can close them. */
    private val sockets = mutableSetOf<Int>()

    /** Reserved by the Shizuku server, which calls it to stop the service. */
    override fun destroy() {
        Log.i(TAG, "destroy")

        synchronized(sockets) {
            sockets.forEach { nativeCloseBus(it) }
            sockets.clear()
        }

        exitProcess(0)
    }

    override fun prepare(libraryDir: String, apkPath: String, abi: String) {
        if (loaded) return

        load(libraryDir, apkPath, abi)
        loaded = true
    }

    override fun openBus(): Int {
        val fd = nativeOpenBus()

        synchronized(sockets) { sockets.add(fd) }

        return fd
    }

    override fun busNode(fd: Int): Int = nativeBusNode(fd)

    override fun send(fd: Int, node: Int, port: Int, data: ByteArray) {
        nativeSend(fd, node, port, data)
    }

    override fun receive(fd: Int, timeoutMs: Int): ByteArray? = nativeReceive(fd, timeoutMs)

    override fun closeBus(fd: Int) {
        synchronized(sockets) { sockets.remove(fd) }
        nativeCloseBus(fd)
    }

    /**
     * Load the library the native calls live in.
     *
     * The build usually ships compressed in the package, so the loader is given
     * the entry inside it; if that is not where it is, it is taken out of the
     * package and loaded from somewhere this process certainly may read.
     */
    private fun load(libraryDir: String, apkPath: String, abi: String) {
        val entry = "lib/$abi/$LIBRARY"
        val candidates = listOf("$libraryDir/$LIBRARY", "$apkPath!/$entry")

        for (path in candidates) {
            try {
                System.load(path)
                Log.i(TAG, "loaded $path")
                return
            } catch (e: Throwable) {
                Log.w(TAG, "could not load $path", e)
            }
        }

        // Nothing else is tried on purpose: this process may run as `shell`,
        // and a library taken out of the package into a directory other
        // processes can write would be a way to have this privilege run
        // somebody else's code.
        throw IllegalStateException("the modem library is not where the package says it is: ${candidates.joinToString()}")
    }

    private external fun nativeOpenBus(): Int
    private external fun nativeBusNode(fd: Int): Int
    private external fun nativeSend(fd: Int, node: Int, port: Int, data: ByteArray): Int
    private external fun nativeReceive(fd: Int, timeoutMs: Int): ByteArray?
    private external fun nativeCloseBus(fd: Int)

    companion object {
        private const val TAG = "QrtrUserService"
        private const val LIBRARY = "librust_lib_nlpa2.so"

        @Volatile
        private var loaded = false
    }
}
