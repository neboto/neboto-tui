# Security groups

A security group is a stateful firewall attached to network interfaces: instances, load balancers, Lambda functions in a VPC, RDS databases. It only allows: traffic no rule allows is dropped, and the reply to allowed traffic is let back in automatically.

## Key ideas

- **Inbound and outbound**: separate rule lists. A new group allows all outbound traffic and no inbound.
- **Stateful**: a reply needs no rule of its own. Network ACLs, on the subnet, are stateless and are evaluated as well.
- **Sources**: a rule names a CIDR range, a prefix list or another security group. A group as the source means "anything carrying that group", wherever its address is.
- **Attachment**: a group does nothing until a network interface carries it. One interface can carry several groups; their rules add up.

## In neboto

- **open to world** (red): an inbound rule allows 0.0.0.0/0 or ::/0. **F** filters to these and **z** sorts them first. Outbound to the world is the AWS default and isn't flagged.
- **unused**: no network interface in this account carries the group. Unused is not safe to delete: another group's rules, a launch template, or an interface in a shared-VPC participant account can still use it, and none of those show here. The VPC's default group is never flagged.
- The pane's sections are Overview, Inbound, Outbound, **Used By** (the network interfaces, fetched when you open it) and Tags. Prefix lists in rules are resolved to their entries.
- The same groups show on @vpc's Security Groups tab, with the same pane.

## Learn more

- Security groups: https://docs.aws.amazon.com/vpc/latest/userguide/vpc-security-groups.html
- Security group rules: https://docs.aws.amazon.com/vpc/latest/userguide/security-group-rules.html
- Compared with network ACLs: https://docs.aws.amazon.com/vpc/latest/userguide/infrastructure-security.html
- Quotas: https://docs.aws.amazon.com/vpc/latest/userguide/amazon-vpc-limits.html
