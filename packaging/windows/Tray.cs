using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Drawing;
using System.IO;
using System.Security.AccessControl;
using System.Security.Principal;
using System.ServiceProcess;
using System.Text;
using System.Web.Script.Serialization;
using System.Windows.Forms;
using System.Runtime.InteropServices;
using Microsoft.Win32;

// Interactive companion only: credentials remain in the elevated setup process.
internal static class TrayApp {
    [DllImport("dwmapi.dll")] static extern int DwmSetWindowAttribute(IntPtr hwnd, int attribute, ref int value, int size);
    static void RoundWindow(IntPtr handle) { int preference = 2; DwmSetWindowAttribute(handle, 33, ref preference, sizeof(int)); }
    static readonly bool Zh = System.Globalization.CultureInfo.CurrentUICulture.Name.StartsWith("zh");
    static string T(string zh, string en) { return Zh ? zh : en; }
    [STAThread] static void Main(string[] args) {
        Application.EnableVisualStyles();
        Application.SetCompatibleTextRenderingDefault(false);
        int build;
        if (!Int32.TryParse(Convert.ToString(Registry.GetValue(@"HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Windows NT\CurrentVersion", "CurrentBuildNumber", "0")), out build) || build < 22000) {
            MessageBox.Show(T("需要 Windows 11 或更高版本。", "Windows 11 or newer is required.")); Environment.ExitCode = 78; return;
        }
        if (args.Length == 1 && (args[0] == "--stop-service" || args[0] == "--start-service")) {
            try { ChangeService(args[0] == "--start-service"); } catch { Environment.ExitCode = 78; }
            return;
        }
        if (args.Length == 1 && args[0] == "--self-test") {
            using (var form = new Setup()) { var handle = form.Handle; }
            using (var menu = new RoundedMenu()) {
                menu.AutoSize = false; menu.Size = new Size(260, 140);
                if (menu.Region == null || menu.Region.IsVisible(0, 0)) Environment.ExitCode = 78;
            }
            return;
        }
        if (args.Length == 2 && args[0] == "--configure-file") {
            try { ImportBootstrap(args[1], false); } catch { Environment.ExitCode = 78; }
            return;
        }
        if (args.Length == 1 && args[0] == "--setup") {
            if (!new WindowsPrincipal(WindowsIdentity.GetCurrent()).IsInRole(WindowsBuiltInRole.Administrator)) {
                MessageBox.Show(T("配置需要管理员权限。", "Setup requires administrator permission.")); return;
            }
            Application.Run(new Setup()); return;
        }
        using (var icon = new NotifyIcon())
        using (var menu = new RoundedMenu()) {
            menu.HandleCreated += delegate { RoundWindow(menu.Handle); };
            icon.Icon = SystemIcons.Application; icon.Text = "Sunshine Client";
            menu.Items.Add(T("查看服务状态", "Service status"), null, delegate { ShowStatus(); });
            menu.Items.Add(T("配对设置…", "Pairing setup…"), null, delegate { Configure(); });
            menu.Items.Add(new ToolStripSeparator());
            menu.Items.Add(T("停止服务并退出", "Stop service and exit"), null, delegate {
                if (ControlService(false)) Application.Exit();
                else MessageBox.Show(T("服务未能停止，托盘保持运行。请完成管理员授权后重试。", "The service could not be stopped; the tray remains open. Allow the administrator prompt and retry."));
            });
            icon.ContextMenuStrip = menu; icon.DoubleClick += delegate { ShowStatus(); };
            icon.Visible = true;
            if (Convert.ToString(Registry.GetValue(@"HKEY_LOCAL_MACHINE\SOFTWARE\sarmg\SunshineClient", "Paired", "0")) == "1") ControlService(true);
            icon.ShowBalloonTip(5000, "Sunshine Client", T("首次使用：右键选择配对设置。", "First use: right-click and choose Pairing setup."), ToolTipIcon.Info);
            Application.Run(); icon.Visible = false;
        }
    }
    sealed class RoundedMenu : ContextMenuStrip {
        protected override void OnSizeChanged(EventArgs e) {
            base.OnSizeChanged(e);
            if (Width < 24 || Height < 24) return;
            using (var path = new System.Drawing.Drawing2D.GraphicsPath()) {
                const int diameter = 16;
                path.AddArc(0, 0, diameter, diameter, 180, 90);
                path.AddArc(Width - diameter, 0, diameter, diameter, 270, 90);
                path.AddArc(Width - diameter, Height - diameter, diameter, diameter, 0, 90);
                path.AddArc(0, Height - diameter, diameter, diameter, 90, 90); path.CloseFigure();
                var old = Region; Region = new Region(path); if (old != null) old.Dispose();
            }
        }
    }
    static void ChangeService(bool start) {
        using (var service = new ServiceController("SunshineClient")) {
            if (start) {
                if (service.Status == ServiceControllerStatus.Stopped) service.Start();
                service.WaitForStatus(ServiceControllerStatus.Running, TimeSpan.FromSeconds(30));
            } else {
                if (service.Status != ServiceControllerStatus.Stopped && service.Status != ServiceControllerStatus.StopPending) service.Stop();
                service.WaitForStatus(ServiceControllerStatus.Stopped, TimeSpan.FromSeconds(30));
            }
        }
    }
    static bool ControlService(bool start) {
        try { ChangeService(start); return true; }
        catch {
            try {
                using (var helper = Process.Start(new ProcessStartInfo(Application.ExecutablePath, start ? "--start-service" : "--stop-service") { UseShellExecute = true, Verb = "runas" })) {
                    helper.WaitForExit(); return helper.ExitCode == 0;
                }
            } catch { return false; }
        }
    }
    static void Configure() {
        try { Process.Start(new ProcessStartInfo(Application.ExecutablePath, "--setup") { UseShellExecute = true, Verb = "runas" }); }
        catch { MessageBox.Show(T("未启动设置；请允许管理员授权后重试。", "Setup was not started. Allow the administrator prompt to continue.")); }
    }
    static void ShowStatus() {
        string status;
        try { using (var service = new ServiceController("SunshineClient")) {
            status = service.Status == ServiceControllerStatus.Running ? T("后台服务正在运行。", "Background service is running.") : T("后台服务未运行。", "Background service is not running.");
        } } catch { status = T("无法读取服务状态。", "Service status is unavailable."); }
        MessageBox.Show(status + "\n\n" + T("这不代表已连接 Manager 或 Sunshine 正常。客户端在线、Sunshine 可达性及配置生效状态请在 Manager 的设备状态页面查看。", "This does not prove Manager connectivity or Sunshine health. Check the Manager device page for client connectivity, Sunshine reachability and configuration effectiveness."), "Sunshine Client");
    }
    static void ProtectState(string path) {
        var attributes = File.GetAttributes(path);
        if ((attributes & FileAttributes.ReparsePoint) != 0) throw new InvalidOperationException();
        bool directory = (attributes & FileAttributes.Directory) != 0;
        FileSystemSecurity acl = directory ? (FileSystemSecurity)new DirectorySecurity() : new FileSecurity();
        acl.SetAccessRuleProtection(true, false);
        acl.SetOwner(new SecurityIdentifier(WellKnownSidType.BuiltinAdministratorsSid, null));
        foreach (var sid in new[] { WellKnownSidType.BuiltinAdministratorsSid, WellKnownSidType.LocalSystemSid })
            acl.AddAccessRule(new FileSystemAccessRule(new SecurityIdentifier(sid, null), FileSystemRights.FullControl,
                directory ? InheritanceFlags.ContainerInherit | InheritanceFlags.ObjectInherit : InheritanceFlags.None, PropagationFlags.None, AccessControlType.Allow));
        if (directory) {
            Directory.SetAccessControl(path, (DirectorySecurity)acl);
            foreach (var child in Directory.GetFileSystemEntries(path)) ProtectState(child);
        } else File.SetAccessControl(path, (FileSecurity)acl);
    }
    static void ImportBootstrap(string bootstrap, bool pair) {
        if (!new WindowsPrincipal(WindowsIdentity.GetCurrent()).IsInRole(WindowsBuiltInRole.Administrator)) throw new InvalidOperationException();
        if (!Path.IsPathRooted(bootstrap) || bootstrap.Contains("\"")) throw new InvalidOperationException();
        string state = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.CommonApplicationData), "SunshineClient");
        var start = new ProcessStartInfo(Path.Combine(AppDomain.CurrentDomain.BaseDirectory, "sunshine-client.exe"), "init --state \"" + state + "\" --bootstrap \"" + bootstrap + "\"") {
            UseShellExecute = false, CreateNoWindow = true, RedirectStandardOutput = true, RedirectStandardError = true
        };
        using (var process = Process.Start(start)) {
            process.BeginOutputReadLine(); process.BeginErrorReadLine(); process.WaitForExit();
            if (process.ExitCode != 0) throw new InvalidOperationException();
        }
        if (pair) {
            start.Arguments = "pair --state \"" + state + "\"";
            using (var process = Process.Start(start)) {
                process.BeginOutputReadLine(); process.BeginErrorReadLine(); process.WaitForExit();
                if (process.ExitCode != 0) throw new InvalidOperationException();
            }
            Registry.SetValue(@"HKEY_LOCAL_MACHINE\SOFTWARE\sarmg\SunshineClient", "Paired", "1");
        }
        // The service's token differs from the interactive administrator's token.
        ProtectState(state);
        using (var service = new ServiceController("SunshineClient")) {
            if (service.Status != ServiceControllerStatus.Running) service.Start();
            service.WaitForStatus(ServiceControllerStatus.Running, TimeSpan.FromSeconds(30));
        }
    }
    sealed class Setup : Form {
        readonly Dictionary<string, TextBox> fields = new Dictionary<string, TextBox>();
        readonly Label result = new Label { AutoSize = true, ForeColor = Color.Firebrick, MaximumSize = new Size(610, 0) };
        readonly Button submit = new Button { AutoSize = true };
        readonly CheckBox restart = new CheckBox { AutoSize = true, MaximumSize = new Size(620, 0) };
        public Setup() {
            Text = T("Sunshine Client · 配对设置", "Sunshine Client · Pairing setup"); Width = 720; Height = 760;
            AutoScaleMode = AutoScaleMode.Dpi; StartPosition = FormStartPosition.CenterScreen;
            var panel = new TableLayoutPanel { Dock = DockStyle.Fill, AutoScroll = true, ColumnCount = 2, Padding = new Padding(16) };
            panel.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 35)); panel.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 65));
            Controls.Add(panel);
            var hint = new Label { AutoSize = true, MaximumSize = new Size(620, 0), Text = T("填写服务器地址和配对码，以及本机 Sunshine 地址、端口和账号密码。只接受系统已信任且名称匹配的 HTTPS 证书。", "Enter the server address and pairing code, plus local Sunshine address, port and credentials. HTTPS certificates must be system-trusted and match the hostname.") };
            panel.Controls.Add(hint); panel.SetColumnSpan(hint, 2);
            Add(panel, "manager_endpoint", T("服务器地址", "Server address"), "", false);
            Add(panel, "enrollment_token", T("配对码", "Pairing code"), "", true);
            Add(panel, "sunshine_endpoint", T("本机 Sunshine 地址", "Local Sunshine address"), "127.0.0.1", false);
            Add(panel, "sunshine_port", T("Sunshine 端口", "Sunshine port"), "47990", false);
            Add(panel, "sunshine_username", T("Sunshine 用户名", "Sunshine username"), "", false);
            Add(panel, "sunshine_password", T("Sunshine 密码", "Sunshine password"), "", true);
            restart.Text = T("允许 Manager 经管理员确认后重启 Sunshine（可能中断串流）", "Allow Manager-confirmed Sunshine restart (may interrupt streaming)");
            panel.Controls.Add(restart); panel.SetColumnSpan(restart, 2);
            panel.Controls.Add(result); panel.SetColumnSpan(result, 2);
            submit.Text = T("保存并启动客户端", "Save and start client"); submit.Click += delegate { Save(); };
            panel.Controls.Add(submit); panel.SetColumnSpan(submit, 2);
        }
        protected override void OnHandleCreated(EventArgs e) { base.OnHandleCreated(e); RoundWindow(Handle); }
        void Add(TableLayoutPanel panel, string key, string label, string value, bool secret) {
            panel.Controls.Add(new Label { Text = label, AutoSize = true });
            var input = new TextBox { Text = value, Dock = DockStyle.Top, UseSystemPasswordChar = secret, MaxLength = 4096 };
            fields.Add(key, input); panel.Controls.Add(input);
        }
        void Save() {
            submit.Enabled = false;
            string temporary = null;
            try {
                var config = new Dictionary<string, object>();
                foreach (var field in fields) {
                    if (String.IsNullOrWhiteSpace(field.Value.Text)) throw new InvalidOperationException();
                    string value = field.Value.Text;
                    if (field.Key == "sunshine_port") continue;
                    config.Add(field.Key, value);
                }
                var manager = new UriBuilder(fields["manager_endpoint"].Text.Contains("://") ? fields["manager_endpoint"].Text : "https://" + fields["manager_endpoint"].Text);
                if (manager.Scheme != "https" && manager.Scheme != "wss") throw new InvalidOperationException();
                manager.Scheme = "wss"; manager.Path = "/sunshine-client/v1/connect";
                config["manager_endpoint"] = manager.Uri.AbsoluteUri;
                int port;
                if (!Int32.TryParse(fields["sunshine_port"].Text, out port) || port < 1 || port > 65535) throw new InvalidOperationException();
                string host = fields["sunshine_endpoint"].Text.Trim();
                // The management adapter remains loopback-only; no remote HTTP relay.
                config["sunshine_endpoint"] = new UriBuilder("https", host, port).Uri.AbsoluteUri;
                config.Add("restart_allowed", restart.Checked);
                var security = new DirectorySecurity();
                security.SetAccessRuleProtection(true, false);
                foreach (var sid in new[] { WellKnownSidType.BuiltinAdministratorsSid, WellKnownSidType.LocalSystemSid })
                    security.AddAccessRule(new FileSystemAccessRule(new SecurityIdentifier(sid, null), FileSystemRights.FullControl, InheritanceFlags.ContainerInherit | InheritanceFlags.ObjectInherit, PropagationFlags.None, AccessControlType.Allow));
                // Create with the restrictive DACL, never write secrets to a public temp file.
                temporary = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.CommonApplicationData), "SunshineClient-setup-" + Guid.NewGuid().ToString("N"));
                Directory.CreateDirectory(temporary, security);
                string bootstrap = Path.Combine(temporary, "bootstrap.json");
                File.WriteAllText(bootstrap, new JavaScriptSerializer().Serialize(config), new UTF8Encoding(false));
                ImportBootstrap(bootstrap, true);
                foreach (var field in fields.Values) field.Clear();
                result.ForeColor = Color.DarkGreen;
                result.Text = T("配对成功，Sunshine 凭据已验证，客户端服务已启动。", "Pairing complete. Sunshine credentials verified; client service started.");
            } catch {
                result.ForeColor = Color.Firebrick;
                result.Text = T("配对未完成。请检查地址、配对码、Sunshine 账号密码、支持的版本及系统证书信任。已有身份不会被覆盖。", "Pairing incomplete. Check addresses, pairing code, Sunshine credentials, supported version and system certificate trust. Existing identity is not overwritten.");
                submit.Enabled = true;
            } finally {
                if (temporary != null) {
                    try { File.Delete(Path.Combine(temporary, "bootstrap.json")); Directory.Delete(temporary); }
                    catch { result.Text += T(" 临时配置未能清除，请联系管理员。", " Temporary configuration cleanup failed; contact your administrator."); }
                }
            }
        }
    }
}
