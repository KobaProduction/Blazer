; Static checks run only the OS PowerShell binary before privileged extraction.
!macro ValidateStatePaths Suffix
  nsExec::ExecToStack `"$PowerShell" -NoLogo -NoProfile -NonInteractive -Command "$$env:PSModulePath=$$PSHOME+'\Modules';$$ErrorActionPreference='Stop';$$s=[Security.Principal.SecurityIdentifier];$$t='S-1-5-18','S-1-5-32-544','S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464';$$p=[Environment]::GetFolderPath('CommonApplicationData');$$v=$$p+'\KobaProduction';$$d=$$v+'\Blazer';foreach($$q in @($$p,$$v,$$d,($$d+'\owner.txt'),($$d+'\staging'),($$d+'\transactions'),($$d+'\logs'))){if(Test-Path -LiteralPath $$q){if((Get-Item -Force -LiteralPath $$q).Attributes-band 1024){throw 'Reparse setup-state path'};if((Get-Acl -LiteralPath $$q).GetOwner($$s).Value-notin $$t){throw 'Untrusted setup-state owner'}}}"`
  Pop $0
  Pop $1
  StrCmp $0 "0" +2
  Goto stage_failed_${Suffix}
  nsExec::ExecToStack `"$PowerShell" -NoLogo -NoProfile -NonInteractive -Command "$$env:PSModulePath=$$PSHOME+'\Modules';$$ErrorActionPreference='Stop';$$s=[Security.Principal.SecurityIdentifier];$$t='S-1-5-18','S-1-5-32-544','S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464';$$v=[Environment]::GetFolderPath('CommonApplicationData')+'\KobaProduction';$$d=$$v+'\Blazer';foreach($$q in @($$v,$$d,($$d+'\owner.txt'),($$d+'\staging'),($$d+'\transactions'),($$d+'\logs'))){if(Test-Path -LiteralPath $$q){foreach($$r in (Get-Acl -LiteralPath $$q).GetAccessRules($$true,$$true,$$s)){if($$r.AccessControlType-eq 0-and($$r.FileSystemRights-band 0x500D0156)-and-not($$r.PropagationFlags-band 2)-and $$r.IdentityReference.Value-notin $$t){throw 'Writable setup-state path'}}}}"`
  Pop $0
  Pop $1
  StrCmp $0 "0" +2
  Goto stage_failed_${Suffix}
!macroend

!macro ValidateStagingRoot Suffix
  !insertmacro ValidateStatePaths ${Suffix}
  ; Create new owned roots with the descriptor atomically. An existing raced
  ; directory is never repaired/adopted: validate its actual ACL again below.
  nsExec::ExecToStack `"$PowerShell" -NoLogo -NoProfile -NonInteractive -Command "$$env:PSModulePath=$$PSHOME+'\Modules';$$ErrorActionPreference='Stop';$$v=[Environment]::GetFolderPath('CommonApplicationData')+'\KobaProduction';$$d=$$v+'\Blazer';$$a=[Security.AccessControl.DirectorySecurity]::new();$$a.SetSecurityDescriptorSddlForm('O:BAG:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;GRGX;;;BU)');foreach($$q in @($$v,$$d)){if(-not [IO.Directory]::Exists($$q)){[void][IO.Directory]::CreateDirectory($$q,$$a)}}"`
  Pop $0
  Pop $1
  StrCmp $0 "0" +2
  Goto stage_failed_${Suffix}
  !insertmacro ValidateStatePaths ${Suffix}
  nsExec::ExecToStack `"$PowerShell" -NoLogo -NoProfile -NonInteractive -Command "$$env:PSModulePath=$$PSHOME+'\Modules';$$ErrorActionPreference='Stop';$$d=[Environment]::GetFolderPath('CommonApplicationData')+'\KobaProduction\Blazer';$$m=$$d+'\owner.txt';if([IO.File]::Exists($$m)){if([IO.File]::ReadAllText($$m)-cne 'KobaProduction.Blazer.Windows/1'){throw 'Foreign setup-state marker'}}elseif(@(Get-ChildItem -Force -LiteralPath $$d).Count){throw 'Unowned setup-state directory'}else{[IO.File]::WriteAllText($$m,'KobaProduction.Blazer.Windows/1')}"`
  Pop $0
  Pop $1
  StrCmp $0 "0" +2
  Goto stage_failed_${Suffix}
!macroend