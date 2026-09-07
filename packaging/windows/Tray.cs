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

// Interactive companion only: credentials remain in the elevated setup process.
internal static class TrayApp {
    static readonly bool Zh = System.Globalization.CultureInfo.CurrentUICulture.Name.StartsWith("zh");
    static string T(string zh, string en) { return Zh ? zh : en; }
    [STAThread] static void Main(string[] args) {
        Application.EnableVisualStyles();
        Application.SetCompatibleTextRenderingDefault(false);
        if (args.Length == 1 && args[0] == "--self-test") { using (var form = new Setup()) { form.CreateControl(); } return; }
        if (args.Length == 2 && args[0] == "--configure-file") {
            try { ImportBootstrap(args[1]); } catch { Environment.ExitCode = 78; }
            return;
        }
        if (args.Length == 1 && args[0] == "--setup") {
            if (!new WindowsPrincipal(WindowsIdentity.GetCurrent()).IsInRole(WindowsBuiltInRole.Administrator)) {
                MessageBox.Show(T("配置需要管理员权限。", "Setup requires administrator permission.")); return;
            }
            Application.Run(new Setup()); return;
        }
        using (var icon = new NotifyIcon())
        using (var menu = new ContextMenuStrip()) {
            icon.Icon = SystemIcons.Application; icon.Text = "Sunshine Client";
            menu.Items.Add(T("查看服务状态", "Service status"), null, delegate { ShowStatus(); });
            menu.Items.Add(T("配对设置…", "Pairing setup…"), null, delegate { Configure(); });
            menu.Items.Add(new ToolStripSeparator());
            menu.Items.Add(T("退出托盘（后台服务继续运行）", "Exit tray (service keeps running)"), null, delegate { Application.Exit(); });
            icon.ContextMenuStrip = menu; icon.DoubleClick += delegate { ShowStatus(); };
            icon.Visible = true;
            icon.ShowBalloonTip(5000, "Sunshine Client", T("首次使用：右键选择配对设置。", "First use: right-click and choose Pairing setup."), ToolTipIcon.Info);
            Application.Run(); icon.Visible = false;
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
    static void ImportBootstrap(string bootstrap) {
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
        public Setup() {
            Text = T("Sunshine Client · 配对设置", "Sunshine Client · Pairing setup"); Width = 720; Height = 760;
            AutoScaleMode = AutoScaleMode.Dpi; StartPosition = FormStartPosition.CenterScreen;
            var panel = new TableLayoutPanel { Dock = DockStyle.Fill, AutoScroll = true, ColumnCount = 2, Padding = new Padding(16) };
            panel.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 35)); panel.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 65));
            Controls.Add(panel);
            var hint = new Label { AutoSize = true, MaximumSize = new Size(620, 0), Text = T("从 Manager 新建实例后复制标识与配对码。选择经过核验的 CA 证书；Sunshine 用户名和密码仅保存在本机。默认不授权重启。", "Create an instance in Manager and copy its IDs and pairing code. Select verified CA certificates. Sunshine credentials stay on this computer. Restart permission is disabled by default.") };
            panel.Controls.Add(hint); panel.SetColumnSpan(hint, 2);
            Add(panel, "manager_endpoint", T("Manager WSS 地址", "Manager WSS URL"), "wss://manager.example.org/sunshine-client/v1/connect", false);
            Add(panel, "manager_id", T("Manager 标识", "Manager ID"), "", false);
            Add(panel, "device_id", T("设备标识", "Device ID"), "", false);
            Add(panel, "enrollment_token", T("配对码", "Pairing code"), "", true);
            Certificate(panel, "manager_ca_pem", T("Manager CA 证书", "Manager CA certificate"));
            Add(panel, "sunshine_endpoint", T("本机 Sunshine HTTPS 地址", "Local Sunshine HTTPS URL"), "https://127.0.0.1:47990/", false);
            Certificate(panel, "sunshine_ca_pem", T("Sunshine CA 证书", "Sunshine CA certificate"));
            Add(panel, "sunshine_username", T("Sunshine 用户名", "Sunshine username"), "", false);
            Add(panel, "sunshine_password", T("Sunshine 密码", "Sunshine password"), "", true);
            panel.Controls.Add(result); panel.SetColumnSpan(result, 2);
            submit.Text = T("保存并启动客户端", "Save and start client"); submit.Click += delegate { Save(); };
            panel.Controls.Add(submit); panel.SetColumnSpan(submit, 2);
        }
        void Add(TableLayoutPanel panel, string key, string label, string value, bool secret) {
            panel.Controls.Add(new Label { Text = label, AutoSize = true });
            var input = new TextBox { Text = value, Dock = DockStyle.Top, UseSystemPasswordChar = secret, MaxLength = 4096 };
            fields.Add(key, input); panel.Controls.Add(input);
        }
        void Certificate(TableLayoutPanel panel, string key, string label) {
            Add(panel, key, label, "", false);
            fields[key].ReadOnly = true;
            var button = new Button { Text = T("选择 PEM 文件…", "Choose PEM file…"), AutoSize = true };
            button.Click += delegate { using (var dialog = new OpenFileDialog { Filter = "PEM certificate|*.pem;*.crt|All files|*.*", CheckFileExists = true }) {
                if (dialog.ShowDialog(this) == DialogResult.OK) fields[key].Text = dialog.FileName;
            } };
            panel.Controls.Add(button); panel.SetColumnSpan(button, 2);
        }
        void Save() {
            submit.Enabled = false;
            string temporary = null;
            try {
                var config = new Dictionary<string, object>();
                foreach (var field in fields) {
                    if (String.IsNullOrWhiteSpace(field.Value.Text)) throw new InvalidOperationException();
                    string value = field.Value.Text;
                    if (field.Key.EndsWith("_ca_pem")) {
                        var file = new FileInfo(value);
                        if (!file.Exists || file.Length > 65536) throw new InvalidOperationException();
                        value = File.ReadAllText(value);
                    }
                    config.Add(field.Key, value);
                }
                config.Add("restart_allowed", false);
                var security = new DirectorySecurity();
                security.SetAccessRuleProtection(true, false);
                foreach (var sid in new[] { WellKnownSidType.BuiltinAdministratorsSid, WellKnownSidType.LocalSystemSid })
                    security.AddAccessRule(new FileSystemAccessRule(new SecurityIdentifier(sid, null), FileSystemRights.FullControl, InheritanceFlags.ContainerInherit | InheritanceFlags.ObjectInherit, PropagationFlags.None, AccessControlType.Allow));
                // Create with the restrictive DACL, never write secrets to a public temp file.
                temporary = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.CommonApplicationData), "SunshineClient-setup-" + Guid.NewGuid().ToString("N"));
                Directory.CreateDirectory(temporary, security);
                string bootstrap = Path.Combine(temporary, "bootstrap.json");
                File.WriteAllText(bootstrap, new JavaScriptSerializer().Serialize(config), new UTF8Encoding(false));
                ImportBootstrap(bootstrap);
                foreach (var field in fields.Values) field.Clear();
                result.ForeColor = Color.DarkGreen;
                result.Text = T("配置已保存，服务已启动。请在 Manager 核验配对和 Sunshine 状态。", "Configuration saved; service started. Verify pairing and Sunshine status in Manager.");
            } catch {
                result.ForeColor = Color.Firebrick;
                result.Text = T("设置未完成。请检查必填项、证书、标识及配对码；已有身份不会被覆盖。", "Setup did not complete. Check required fields, certificates, IDs and pairing code. Existing identity is not overwritten.");
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
