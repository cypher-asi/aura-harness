//! [`VmProvider`] for [`BoxdProvider`]: one boxd call per method, on the
//! home endpoint or where the machine lives.

use std::time::Duration;

use async_trait::async_trait;

use super::wire::{machine_info, millis_from, set_secret_request, snapshot_info, vm_usage};
use super::{BoxdProvider, CALL_TIMEOUT, CREATE_TIMEOUT};
use crate::error::{CloudError, CloudResult};
use crate::proto;
use crate::provider::{
    Balance, ExecOutput, HostSecret, MachineInfo, MachineLimit, MachineOrigin, NewMachine,
    OrgUsage, SnapshotInfo, UsageReading, VmProvider,
};

#[async_trait]
impl VmProvider for BoxdProvider {
    async fn create(&self, spec: &NewMachine, origin: &MachineOrigin) -> CloudResult<MachineInfo> {
        let mut size = spec.size;
        loop {
            match self.create_once(spec, origin, size).await {
                Err(err) if *origin == MachineOrigin::Fresh && err.is_size_refused() => {
                    let Some(smaller) = size.smaller() else {
                        return Err(err);
                    };
                    tracing::warn!(
                        machine = %spec.name,
                        refused = %size,
                        fallback = %smaller,
                        "boxd refused the machine size; trying one size down"
                    );
                    size = smaller;
                }
                other => return other,
            }
        }
    }

    async fn get(&self, machine: &str) -> CloudResult<Option<MachineInfo>> {
        let request = proto::GetVmRequest {
            vm_id: machine.to_string(),
        };
        match self
            .unary(
                "GetVm",
                &self.endpoint_of(machine),
                request,
                CALL_TIMEOUT,
                |mut c, r| async move { c.get_vm(r).await },
            )
            .await
        {
            Ok(vm) => {
                let info = machine_info(vm);
                self.remember(&info);
                Ok(Some(info))
            }
            Err(err) if err.is_not_found() => Ok(None),
            Err(err) => Err(err),
        }
    }

    async fn list(&self, label_selector: &[String]) -> CloudResult<Vec<MachineInfo>> {
        let request = proto::ListVmsRequest {
            org: self.org.clone(),
            label_selector: label_selector.to_vec(),
            ..Default::default()
        };
        let answer = self
            .unary(
                "ListVms",
                &self.create_endpoint(),
                request,
                CALL_TIMEOUT,
                |mut c, r| async move { c.list_vms(r).await },
            )
            .await?;
        let machines: Vec<MachineInfo> = answer.vms.into_iter().map(machine_info).collect();
        for machine in &machines {
            self.remember(machine);
        }
        Ok(machines)
    }

    async fn destroy(&self, machine: &str) -> CloudResult<()> {
        let request = proto::DestroyVmRequest {
            vm_id: machine.to_string(),
        };
        match self
            .unary(
                "DestroyVm",
                &self.endpoint_of(machine),
                request,
                CALL_TIMEOUT,
                |mut c, r| async move { c.destroy_vm(r).await },
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(err) if err.is_not_found() => Ok(()),
            Err(err) => Err(err),
        }
    }

    async fn hibernate(&self, machine: &str) -> CloudResult<()> {
        let request = proto::HibernateVmRequest {
            vm_id: machine.to_string(),
        };
        self.unary(
            "HibernateVm",
            &self.endpoint_of(machine),
            request,
            CALL_TIMEOUT,
            |mut c, r| async move { c.hibernate_vm(r).await },
        )
        .await
        .map(|_| ())
    }

    async fn wake(&self, machine: &str) -> CloudResult<()> {
        let request = proto::WakeVmRequest {
            vm_id: machine.to_string(),
        };
        self.unary(
            "WakeVm",
            &self.endpoint_of(machine),
            request,
            CALL_TIMEOUT,
            |mut c, r| async move { c.wake_vm(r).await },
        )
        .await
        .map(|_| ())
    }

    async fn standby(&self, machine: &str) -> CloudResult<()> {
        let request = proto::SuspendVmRequest {
            vm_id: machine.to_string(),
        };
        self.unary(
            "SuspendVm",
            &self.endpoint_of(machine),
            request,
            CALL_TIMEOUT,
            |mut c, r| async move { c.suspend_vm(r).await },
        )
        .await
        .map(|_| ())
    }

    async fn set_auto_hibernate(&self, machine: &str, secs: u32) -> CloudResult<()> {
        let request = proto::SetAutoHibernateTimeoutRequest {
            vm_id: machine.to_string(),
            timeout_secs: secs,
        };
        self.unary(
            "SetAutoHibernateTimeout",
            &self.endpoint_of(machine),
            request,
            CALL_TIMEOUT,
            |mut c, r| async move { c.set_auto_hibernate_timeout(r).await },
        )
        .await
        .map(|_| ())
    }

    async fn set_bot_protection(&self, machine: &str, enabled: bool) -> CloudResult<()> {
        let request = proto::SetBotProtectionRequest {
            vm_id: machine.to_string(),
            enabled,
            mode: if enabled { "on" } else { "off" }.to_string(),
        };
        self.unary(
            "SetBotProtection",
            &self.endpoint_of(machine),
            request,
            CALL_TIMEOUT,
            |mut c, r| async move { c.set_bot_protection(r).await },
        )
        .await
        .map(|_| ())
    }

    async fn exec(
        &self,
        machine: &str,
        command: &str,
        stdin: Option<Vec<u8>>,
        timeout: Duration,
    ) -> CloudResult<ExecOutput> {
        self.exec_streaming(machine, command, stdin, timeout, &mut |_, _| {})
            .await
    }

    async fn exec_streaming(
        &self,
        machine: &str,
        command: &str,
        stdin: Option<Vec<u8>>,
        timeout: Duration,
        on_chunk: &mut (dyn for<'chunk> FnMut(bool, &'chunk [u8]) + Send),
    ) -> CloudResult<ExecOutput> {
        self.exec_stream(machine, command, stdin, timeout, on_chunk)
            .await
    }

    async fn upload(&self, machine: &str, path: &str, bytes: Vec<u8>) -> CloudResult<u64> {
        self.upload_reported(machine, path, bytes, &mut |_, _| {})
            .await
    }

    async fn upload_reported(
        &self,
        machine: &str,
        path: &str,
        bytes: Vec<u8>,
        on_sent: &mut (dyn FnMut(u64, u64) + Send),
    ) -> CloudResult<u64> {
        self.upload_stream(machine, path, bytes, on_sent).await
    }

    async fn download(&self, machine: &str, path: &str, max_bytes: u64) -> CloudResult<Vec<u8>> {
        self.download_stream(machine, path, max_bytes).await
    }

    async fn save_snapshot(&self, machine: &str, name: &str) -> CloudResult<SnapshotInfo> {
        let request = proto::CreateSnapshotRequest {
            vm: machine.to_string(),
            name: name.to_string(),
        };
        let answer = self
            .unary(
                "CreateSnapshot",
                &self.endpoint_of(machine),
                request,
                CREATE_TIMEOUT,
                |mut c, r| async move { c.create_snapshot(r).await },
            )
            .await?;
        Ok(SnapshotInfo {
            id: answer.snapshot_id,
            name: answer.name,
            version: answer.version,
            status: answer.status,
            size_bytes: 0,
            vcpu: 0,
        })
    }

    async fn snapshot(&self, name: &str) -> CloudResult<Option<SnapshotInfo>> {
        let request = proto::GetSnapshotRequest {
            name: name.to_string(),
            org: self.org.clone(),
        };
        match self
            .unary(
                "GetSnapshot",
                &self.create_endpoint(),
                request,
                CALL_TIMEOUT,
                |mut c, r| async move { c.get_snapshot(r).await },
            )
            .await
        {
            Ok(answer) => Ok(answer.snapshot.map(snapshot_info)),
            Err(err) if err.is_not_found() => Ok(None),
            Err(err) => Err(err),
        }
    }

    async fn delete_snapshot(&self, name: &str) -> CloudResult<()> {
        let request = proto::DeleteSnapshotRequest {
            name: name.to_string(),
            org: self.org.clone(),
        };
        match self
            .unary(
                "DeleteSnapshot",
                &self.create_endpoint(),
                request,
                CALL_TIMEOUT,
                |mut c, r| async move { c.delete_snapshot(r).await },
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(err) if err.is_not_found() => Ok(()),
            Err(err) => Err(err),
        }
    }

    async fn machine_limit(&self) -> CloudResult<MachineLimit> {
        let request = proto::GetOrgMachineLimitRequest {
            org_id: self.org.clone(),
        };
        let answer = self
            .unary(
                "GetOrgMachineLimit",
                &self.create_endpoint(),
                request,
                CALL_TIMEOUT,
                |mut c, r| async move { c.get_org_machine_limit(r).await },
            )
            .await?;
        Ok(MachineLimit {
            count: answer.vm_count,
            max: answer.max_vms,
        })
    }

    async fn machine_usage(&self, machine: &str) -> CloudResult<UsageReading> {
        const OP: &str = "GetVmUsage";
        let request = proto::GetVmUsageRequest {
            vm_id: machine.to_string(),
            org: self.org.clone(),
            ..Default::default()
        };
        let answer = self
            .unary(
                OP,
                &self.home(),
                request,
                CALL_TIMEOUT,
                |mut c, r| async move { c.get_vm_usage(r).await },
            )
            .await?;
        let until_ms = millis_from(answer.period_end);
        let usage = answer.usage.ok_or(CloudError::InvalidResponse {
            op: OP,
            message: "no usage in the answer".into(),
        })?;
        Ok(UsageReading {
            usage: vm_usage(&usage, until_ms),
            currency: answer.currency,
        })
    }

    async fn org_usage(&self, since_ms: u64) -> CloudResult<OrgUsage> {
        let request = proto::GetOrgUsageRequest {
            org: self.org.clone(),
            since: i64::try_from(since_ms / 1000).unwrap_or(0),
            until: 0,
        };
        let answer = self
            .unary(
                "GetOrgUsage",
                &self.home(),
                request,
                CALL_TIMEOUT,
                |mut c, r| async move { c.get_org_usage(r).await },
            )
            .await?;
        let until_ms = millis_from(answer.period_end);
        Ok(OrgUsage {
            total_cost_micro: answer.total_cost_micro,
            currency: answer.currency,
            complete: answer.complete,
            machines: answer
                .vms
                .iter()
                .map(|vm| (vm.name.clone(), vm_usage(vm, until_ms)))
                .collect(),
        })
    }

    async fn set_secret(&self, secret: &HostSecret) -> CloudResult<()> {
        let org = self.secret_org().await?;
        let request = set_secret_request(secret, &org);
        self.unary(
            "SetVar",
            &self.home(),
            request,
            CALL_TIMEOUT,
            |mut c, r| async move { c.set_var(r).await },
        )
        .await?;
        Ok(())
    }

    async fn remove_secret(&self, name: &str) -> CloudResult<()> {
        let org = self.secret_org().await?;
        let request = proto::RemoveVarRequest {
            org,
            kind: "secret".into(),
            name: name.to_string(),
            scope: "private-unattached".into(),
        };
        match self
            .unary(
                "RemoveVar",
                &self.home(),
                request,
                CALL_TIMEOUT,
                |mut c, r| async move { c.remove_var(r).await },
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(err) if err.is_not_found() => Ok(()),
            Err(err) => Err(err),
        }
    }

    async fn balance(&self) -> CloudResult<Balance> {
        let request = proto::ListBillingActivityRequest {
            org: self.org.clone(),
            page_size: 1,
            ..Default::default()
        };
        let answer = self
            .unary(
                "ListBillingActivity",
                &self.home(),
                request,
                CALL_TIMEOUT,
                |mut c, r| async move { c.list_billing_activity(r).await },
            )
            .await?;
        Ok(Balance {
            currency: answer.currency,
            balance_micro: answer.balance_micro,
        })
    }
}
