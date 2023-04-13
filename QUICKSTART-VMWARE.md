# Using Bottlerocket as a Kubernetes worker node with VMware

This quickstart will walk through joining a Bottlerocket VM guest to an existing Kubernetes cluster running in VMware.

## Prerequisites

You must be able to access vSphere, via webUI or some type of client.
We will use the CLI tool [`govc`](https://github.com/vmware/govmomi/tree/master/govc) to communicate with vSphere in this guide.
`govc` can use [environment variables or take arguments](https://github.com/vmware/govmomi/tree/master/govc#usage) to specify needed parameters.
For the purposes of this guide we will assume that the following environment variables are set to the proper values in your environment:

```
GOVC_URL
GOVC_USERNAME
GOVC_PASSWORD
GOVC DATACENTER
GOVC_DATASTORE
GOVC_NETWORK
GOVC_RESOURCE_POOL
GOVC_FOLDER
```

This guide assumes you already have a functioning Kubernetes cluster running in VMware.
You'll need to have [`kubectl`](https://kubernetes.io/docs/tasks/tools/#kubectl) and [`kubeadm`](https://kubernetes.io/docs/tasks/tools/#kubeadm) installed, as well as a `kubeconfig` for your cluster.
These tools allow us to access information about your cluster to facilitate the joining of Bottlerocket nodes.

You'll need to install [`tuftool`](https://github.com/awslabs/tough/blob/develop/tuftool/README.md) to assist you with fetching the signed Bottlerocket OVA from the Bottlerocket TUF repository.

`jq` should also be installed.

If you'd (optionally) like to make use of the control container, you'll need an AWS account and AWS CLI.

## Fetch the OVA

The Bottlerocket OVA is signed and uploaded alongside the rest of the Bottlerocket release artifacts.

You first need the Bottlerocket root role, which is used by `tuftool` to verify the OVA.
The following will download and verify the root role itself:

```shell
curl -O "https://cache.bottlerocket.aws/root.json"
sha512sum -c <<<"b81af4d8eb86743539fbc4709d33ada7b118d9f929f0c2f6c04e1d41f46241ed80423666d169079d736ab79965b4dd25a5a6db5f01578b397496d49ce11a3aa2  root.json"
```

Next, set your desired version and variant, and download the OVA:

```shell
VERSION="v1.6.1"
VARIANT="vmware-k8s-1.24"
OVA="bottlerocket-${VARIANT}-x86_64-${VERSION}.ova"
OUTDIR="${VARIANT}-${VERSION}"

tuftool download "${OUTDIR}" --target-name "${OVA}" \
   --root ./root.json \
   --metadata-url "https://updates.bottlerocket.aws/2020-07-07/${VARIANT}/x86_64/" \
   --targets-url "https://updates.bottlerocket.aws/targets/"
```

## Upload the OVA

Once you have downloaded the OVA, you can upload it to vSphere.

The first command generates a spec file (`bottlerocket_spec.json` in this case) using the OVA and gives you few options for your deployment.

```shell
govc import.spec "${OUTDIR}/${OVA}" > bottlerocket_spec.json
```

The spec will look similar to this:

```json
{
  "DiskProvisioning": "flat",
  "IPAllocationPolicy": "dhcpPolicy",
  "IPProtocol": "IPv4",
  "NetworkMapping": [
    {
      "Name": "VM Network",
      "Network": ""
    }
  ],
  "MarkAsTemplate": false,
  "PowerOn": false,
  "InjectOvfEnv": false,
  "WaitForIP": false,
  "Name": null
}
```

We will use `$GOVC_NETWORK` to populate the value for `Network` in the file and use it to upload the OVA!

```shell
VM_NAME="bottlerocket-quickstart-${VERSION}"

jq --arg network "${GOVC_NETWORK}" \
  '.NetworkMapping[].Network = $network' \
  bottlerocket_spec.json > bottlerocket_spec_edit.json

govc import.ova -options=bottlerocket_spec_edit.json -name="${VM_NAME}" "${OUTDIR}/${OVA}"
```

Since we intend to run multiple identical VMs, let's mark the OVA you just uploaded as a template.
You can think of a template as a "golden" image, allowing you to create many VMs without affecting the "golden" image.

```shell
govc vm.markastemplate "${VM_NAME}"
```

Let's create 3 Bottlerocket VMs using the template.
The following will clone from the template, but leave the VMs turned off since we still need to set user data.

```shell
for node in 1 2 3; do
  govc vm.clone -vm "${VM_NAME}" -on=false "${VM_NAME}-${node}"
done
```

## Gathering cluster info

This section will help you gather cluster information needed to configure Bottlerocket via user data.
The below commands assume a single cluster.

#### API Server
This is the address (including port) of the control plane.

```shell
export API_SERVER="$(kubectl config view -o jsonpath='{.clusters[0].cluster.server}')"
```

#### Cluster DNS IP
This is the IP address of the DNS pod/service.

```shell
export CLUSTER_DNS_IP="$(kubectl -n kube-system get svc -l k8s-app=kube-dns -o=jsonpath='{.items[0].spec.clusterIP}')"
```

#### Bootstrap token
Nodes require a token to establish trust between the node and the control plane.
Tokens must be [used within 24 hours](https://kubernetes.io/docs/reference/setup-tools/kubeadm/kubeadm-token/), but once the node has booted and registered with the cluster, it isn't used again.

```shell
export BOOTSTRAP_TOKEN="$(kubeadm token create)"
```

#### Cluster Certificate
This is the base64 encoded cluster certificate authority.

```shell
export CLUSTER_CERTIFICATE="$(kubectl config view --raw -o=jsonpath='{.clusters[0].cluster.certificate-authority-data}')"
```

## Configuring Bottlerocket

In order to join Bottlerocket to your cluster, it must be configured via user data.
There are multiple methods of passing user data to Bottlerocket in VMware; we will demonstrate all of them.

Create a file called `user-data.toml` and populate it with the values you just retrieved.

```shell
cat <<EOF > user-data.toml
[settings.kubernetes]
api-server = "${API_SERVER}"
cluster-dns-ip = "${CLUSTER_DNS_IP}"
bootstrap-token = "${BOOTSTRAP_TOKEN}"
cluster-certificate = "${CLUSTER_CERTIFICATE}"
EOF
```

### Accessing your Bottlerocket guest via host containers
For remote access to your running Bottlerocket VMs, you will need to add additional user data to enable host containers.
The Bottlerocket VMware images don't include any host containers enabled by default.
But don't worry!
You can use our [admin](https://github.com/bottlerocket-os/bottlerocket-admin-container) and/or [control](https://github.com/bottlerocket-os/bottlerocket-control-container) containers, they just need to be configured first.

#### Admin container
If you would like to use the admin container, you will need to create some base64 encoded user data which will be passed to the container at runtime.
Full details are covered in the [admin container documentation](https://github.com/bottlerocket-os/bottlerocket-admin-container#authenticating-with-the-admin-container).
If we assume you have a public key at `${HOME}/.ssh/id_rsa.pub`, the below will add the correct user data to your `user-data.toml`.

```shell
PUBKEY="${HOME}/.ssh/id_rsa.pub"
ADMIN_USER_DATA="$(echo '{"ssh":{"authorized-keys":["'"$(cat ${PUBKEY})"'"]}}' | base64 -w 0)"

cat <<EOF >>user-data.toml
[settings.host-containers.admin]
enabled = true
user-data = "${ADMIN_USER_DATA}"
EOF
```

#### Control container
Enabling the control container is very similar to the admin container; you will create some base64 encoded user data that will be passed to the container at runtime.
This user data includes an activation ID and code retrieved from AWS SSM.
Full details can be found in the [control container documentation](https://github.com/bottlerocket-os/bottlerocket-control-container#connecting-to-aws-systems-manager-ssm).

You'll first need an AWS account, and AWS CLI installed.
Then you'll create a service role in that account to [grant AWS STS trust to the AWS Systems Manager service](https://docs.aws.amazon.com/systems-manager/latest/userguide/sysman-service-role.html).

```shell
cat <<EOF > ssmservice-trust.json
{
    "Version": "2012-10-17",
    "Statement": {
        "Effect": "Allow",
        "Principal": {
            "Service": "ssm.amazonaws.com"
        },
        "Action": "sts:AssumeRole"
    }
}
EOF

# Create the role using the above policy
aws iam create-role \
    --role-name SSMServiceRole \
    --assume-role-policy-document file://ssmservice-trust.json

# Attach the policy enabling the role to create session tokens
aws iam attach-role-policy \
    --role-name SSMServiceRole \
    --policy-arn arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore
```

Once the above is created, we can use the role to create an activation code and ID.

```shell
export SSM_ACTIVATION="$(aws ssm create-activation \
  --iam-role SSMServiceRole \
  --registration-limit 100 \
  --region us-west-2 \
  --output json)"
```

Using the above activation data we can create our user data to pass to the control container:

```shell
SSM_ACTIVATION_ID="$(jq -r '.ActivationId' <<< ${SSM_ACTIVATION})"
SSM_ACTIVATION_CODE="$(jq -r '.ActivationCode' <<< ${SSM_ACTIVATION})"
CONTROL_USER_DATA="$(echo '{"ssm":{"activation-id":"'${SSM_ACTIVATION_ID}'","activation-code":"'${SSM_ACTIVATION_CODE}'","region":"us-west-2"}}' | base64 -w0)"

cat <<EOF >>user-data.toml
[settings.host-containers.control]
enabled = true
user-data = "${CONTROL_USER_DATA}"
EOF
```

### Setting user data via "guestinfo" interface
**Note: You must set these values before you start the VM for the first time!**

VMware allows you to set some extended attributes of your VM, which your VM can then access via a "guestinfo" interface.
These extended attributes are `guestinfo.userdata` and `guestinfo.userdata.encoding`.

`guestinfo.userdata` may be passed as base64, gzipped base64, or (least desirable) raw TOML.
Valid values for `guestinfo.userdata.encoding` are: `base64`, `b64`, `gzip+base64`, and `gz+b64`.

Given the above file `user-data.toml`, base64 encode and set user data for your VM:
```shell
export BR_USERDATA=$(base64 -w0 user-data.toml)

for node in 1 2 3; do
  govc vm.change -vm "${VM_NAME}-${node}" \
    -e guestinfo.userdata="${BR_USERDATA}" \
    -e guestinfo.userdata.encoding="base64"
done
```

You can check that your user data was set; using node "1" as an example below:
```shell
govc vm.info -e -r -t "${VM_NAME}-1"
```

## Launch!
Once you've created your user data and given your VM a way to access it via guestinfo, you can launch all 3 Bottlerocket VMs in your cluster!
```shell
for node in 1 2 3; do
  govc vm.power -on "${VM_NAME}-${node}"
done
```

Once it launches, you should be able to use your Bottlerocket instance using normal Kubernetes workflows.
All boot output should be visible in the vSphere console if you need to troubleshoot.
