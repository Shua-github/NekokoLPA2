package ee.nekoko.nlpa2

import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine

class MainActivity : FlutterActivity() {
    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        flutterEngine.plugins.add(NBridgePlugin())
        flutterEngine.plugins.add(OmapiPlugin())
        flutterEngine.plugins.add(TelephonyPlugin())
        flutterEngine.plugins.add(CertificatePlugin())
        flutterEngine.plugins.add(QrtrPlugin())
    }
}
