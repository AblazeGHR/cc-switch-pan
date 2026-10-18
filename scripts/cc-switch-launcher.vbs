Option Explicit

Dim shell, fso, root, configPath, showConsole, line, config, gui, args
Set shell = CreateObject("WScript.Shell")
Set fso = CreateObject("Scripting.FileSystemObject")
root = fso.GetParentFolderName(WScript.ScriptFullName)
configPath = fso.BuildPath(root, "cc-switch-launcher.ini")
showConsole = False

If fso.FileExists(configPath) Then
  Set config = fso.OpenTextFile(configPath, 1, False)
  Do Until config.AtEndOfStream
    line = LCase(Trim(config.ReadLine))
    If line = "show_console=true" Or line = "showconsole=true" Then showConsole = True
  Loop
  config.Close
End If

gui = fso.BuildPath(root, "cc-switch-gui.exe")
args = ""
If WScript.Arguments.Count > 0 Then
  Dim i
  For i = 0 To WScript.Arguments.Count - 1
    args = args & " " & Chr(34) & WScript.Arguments(i) & Chr(34)
  Next
End If

' 0 = hidden, 1 = normal window. The GUI process itself remains visible;
' only the inherited console window is controlled here.
If showConsole Then
  shell.Run Chr(34) & gui & Chr(34) & args, 1, False
Else
  shell.Run Chr(34) & gui & Chr(34) & args, 0, False
End If
