$env:PSModulePath = $PSHOME + '\Modules'
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$adapters = @(Get-CimInstance -Namespace root/StandardCimv2 -ClassName MSFT_NetAdapter -Property InterfaceIndex,Name,InterfaceDescription,InterfaceType,MediaConnectState,InterfaceOperationalStatus,HardwareInterface,ComponentID,DriverName | ForEach-Object {
    [ordered]@{ index=[uint32]$_.InterfaceIndex; name=[string]$_.Name; description=[string]$_.InterfaceDescription; interface_type=[uint32]$_.InterfaceType; connected=($_.MediaConnectState -eq 1 -and $_.InterfaceOperationalStatus -eq 1); hardware=[bool]$_.HardwareInterface; component=[string]$_.ComponentID; driver=[string]$_.DriverName }
})
$routes = @(Get-CimInstance -Namespace root/StandardCimv2 -ClassName MSFT_NetRoute -Filter 'Store=1 AND State=0 AND Protocol<>2' -Property InterfaceIndex,DestinationPrefix | ForEach-Object {
    [ordered]@{ index=[uint32]$_.InterfaceIndex; prefix=[string]$_.DestinationPrefix }
})
$ras = @()
if (Get-Command Get-VpnConnection -ErrorAction SilentlyContinue) {
    $ras = @(@(Get-VpnConnection -ErrorAction Stop) + @(Get-VpnConnection -AllUserConnection -ErrorAction Stop) | Where-Object ConnectionStatus -eq 'Connected' | ForEach-Object { [ordered]@{ name=[string]$_.Name; description=[string]$_.TunnelType } })
}
ConvertTo-Json -Compress -Depth 4 -InputObject ([ordered]@{ adapters=$adapters; routes=$routes; ras=$ras })
