# IAM roles and policies

IAM decides who can call which AWS action on which resource. A role is an identity with no password or long-term keys: something trusted assumes it and gets temporary credentials. A policy is a JSON document of allows and denies, attached to a role, user or group.

## Key ideas

- **Trust policy**: on a role, who may assume it (a service such as ec2.amazonaws.com, another account, a federated identity). It grants no permissions itself.
- **Permission policies**: what the role may do once assumed. **Managed** policies are standalone and reusable (customer- or AWS-managed); **inline** policies live inside one role.
- **Evaluation**: everything is denied by default, an allow grants it, and an explicit deny anywhere wins. SCPs, permission boundaries and resource policies can each narrow what an allow grants.
- **Last accessed**: when each service was last used by a role, so you can tell what it actually needs.

## In neboto

- A role's pane has Trust Policy, Permissions (what's attached), Policies (every document, in full), Tags and Last Accessed. **⏎** on a managed policy opens its pane; on an inline one it moves to that document.
- The Policies tab lists your customer-managed policies plus only the AWS-managed ones **attached to something here**, not all of AWS's 1,400. They read "AWS managed"; search for aws-managed.
- **Tags aren't in the list**: AWS's list calls leave them out, so a tag: search can't match IAM roles or policies. The pane fetches them when you open one.
- Last Accessed starts a report job (a read action AWS includes in read-only access) and waits for it, up to a minute. neboto never generates the account's credential report, which would create one.

## Learn more

- IAM roles: https://docs.aws.amazon.com/IAM/latest/UserGuide/id_roles.html
- Policies and permissions: https://docs.aws.amazon.com/IAM/latest/UserGuide/access_policies.html
- Managed and inline policies: https://docs.aws.amazon.com/IAM/latest/UserGuide/access_policies_managed-vs-inline.html
- Policy evaluation logic: https://docs.aws.amazon.com/IAM/latest/UserGuide/reference_policies_evaluation-logic.html
- Last accessed information: https://docs.aws.amazon.com/IAM/latest/UserGuide/access_policies_last-accessed.html
