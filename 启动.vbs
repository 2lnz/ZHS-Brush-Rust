' 双击启动，全程无控制台黑框（比 启动.bat 更干净）
Option Explicit
Dim sh, fso, base, exePath
Set sh = CreateObject("WScript.Shell")
Set fso = CreateObject("Scripting.FileSystemObject")
base = fso.GetParentFolderName(WScript.ScriptFullName)
exePath = base & "\target\release\zhs-brush-gui.exe"
If Not fso.FileExists(exePath) Then
    MsgBox "还没编译，请先双击 构建.bat", 48, "ZHS-Brush-RUST"
    WScript.Quit 1
End If
sh.CurrentDirectory = base
sh.Run """" & exePath & """ --data-dir """ & base & """", 0, False
