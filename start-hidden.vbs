Set WshShell = CreateObject("WScript.Shell")
WshShell.CurrentDirectory = "C:\Users\evano\repos\loc-dock"
WshShell.Run "uv run dock.py", 0, False
