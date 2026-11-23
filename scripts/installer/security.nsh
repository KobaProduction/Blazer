; These static commands execute only the OS PowerShell binary, before extracting
; any privileged script. Existing parent/root/owner-marker paths must be trusted.
!macro ValidateStagingRoot Suffix
  nsExec::ExecToStack `"$PowerShell" -NoLogo -NoProfile -NonInteractive -Command "$$env:PSModulePath=$$PSHOME+'\Modules';$$ErrorActionPreference='Stop';$$s=[Security.Principal.SecurityIdentifier];$$t='S-1-5-18','S-1-5-32-544','S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464';$$p=[Environment]::GetFolderPath('ProgramFiles');$$d=Join-Path $$p 'Blazer.Setup.Stage';foreach($$q in @($$p,$$d,(Join-Path $$d 'owner.txt'))){if(Test-Path -LiteralPath $$q){if((Get-Item -Force -LiteralPath $$q).Attributes-band 1024){exit 2};if((Get-Acl -LiteralPath $$q).GetOwner($$s).Value-notin $$t){exit 2}}}"`
  Pop $0
  Pop $1
  StrCmp $0 "0" +2
  Goto stage_failed_${Suffix}
  nsExec::ExecToStack `"$PowerShell" -NoLogo -NoProfile -NonInteractive -Command "$$env:PSModulePath=$$PSHOME+'\Modules';$$ErrorActionPreference='Stop';$$s=[Security.Principal.SecurityIdentifier];$$t='S-1-5-18','S-1-5-32-544','S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464';$$p=[Environment]::GetFolderPath('ProgramFiles');$$d=Join-Path $$p 'Blazer.Setup.Stage';foreach($$q in @($$p,$$d,(Join-Path $$d 'owner.txt'))){if(Test-Path -LiteralPath $$q){foreach($$r in (Get-Acl -LiteralPath $$q).GetAccessRules($$true,$$true,$$s)){if($$r.AccessControlType-eq 0-and($$r.FileSystemRights-band 0x500D0156)-and-not($$r.PropagationFlags-band 2)-and $$r.IdentityReference.Value-notin $$t){exit 2}}}}"`
  Pop $0
  Pop $1
  StrCmp $0 "0" +2
  Goto stage_failed_${Suffix}
!macroend
