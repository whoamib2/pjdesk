# Release logging. The default Android optimize rules treat Log.i and Log.println
# as removable. WARN and ERROR have to stay, or a release logcat shows no PJDesk lines.
-keep class us.spader.pjdesk.AppLog { *; }
-keepclassmembers class android.util.Log {
    public static boolean isLoggable(java.lang.String, int);
    public static int w(java.lang.String, java.lang.String);
    public static int e(java.lang.String, java.lang.String);
    public static int e(java.lang.String, java.lang.String, java.lang.Throwable);
}
