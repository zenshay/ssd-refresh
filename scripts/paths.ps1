# This helper also works with Windows PowerShell 5.1 and .NET Framework.
function Get-ChildRelativePath([string]$Base, [string]$Path) {
    $directory = [IO.Path]::GetFullPath($Base).TrimEnd([char[]]@([char]92, [char]47))
    $absolute = [IO.Path]::GetFullPath($Path)
    $prefix = $directory + [IO.Path]::DirectorySeparatorChar
    $comparison = [StringComparison]::Ordinal
    if ([IO.Path]::DirectorySeparatorChar -eq [char]92) {
        $comparison = [StringComparison]::OrdinalIgnoreCase
    }
    if (-not $absolute.StartsWith($prefix, $comparison)) {
        throw "Path is outside the base directory: $Path"
    }
    return $absolute.Substring($prefix.Length).Replace('\', '/')
}
