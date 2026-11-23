# Organization policies

AWS Organizations policies are set once for the organization and attached to its root, to OUs or to accounts. SCPs and RCPs are guardrails: they never grant anything, they only cap what IAM in member accounts can allow. The other types (tag, backup, AI opt-out, chatbot, declarative) set standards instead.

## Key ideas

- **SCP**: the most any principal in an account may do. An action has to be allowed at every level from the root down to the account, and also by the principal's own IAM policies.
- **RCP**: the same idea applied to resources (an S3 bucket, a KMS key), whoever calls them.
- **Inheritance**: a policy attached to an OU applies to everything below it, so most of an account's SCPs come from its OUs, not from the account itself.
- **Management account**: SCPs and RCPs never restrict it.

## In neboto

- This tab lists every enabled policy type; the Type column says which. A policy's pane has its Document and the targets it is attached to.
- On an account or OU, the Policies section shows the **effective** set: what is attached there and everything inherited, each row marked "inherited from" the OU it comes from.
- **⏎** on a target opens that account or OU, and **⏎** on a policy row opens the policy.
- Organizations is read from the management account or a delegated administrator; anywhere else most of this is denied.

## Learn more

- Service control policies: https://docs.aws.amazon.com/organizations/latest/userguide/orgs_manage_policies_scps.html
- How SCPs are evaluated: https://docs.aws.amazon.com/organizations/latest/userguide/orgs_manage_policies_scps_evaluation.html
- Resource control policies: https://docs.aws.amazon.com/organizations/latest/userguide/orgs_manage_policies_rcps.html
- Policy types: https://docs.aws.amazon.com/organizations/latest/userguide/orgs_manage_policies.html
