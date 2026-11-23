# Amazon ECS

Elastic Container Service runs containers for you. You describe a container setup once (a task definition), and ECS starts copies of it on Fargate or on EC2 instances you provide, either as long-running services or as one-off tasks.

## Key ideas

- **Cluster**: a grouping of services and tasks, plus the capacity they run on.
- **Task definition**: the blueprint (images, CPU and memory, ports, IAM roles, logging), versioned as family:revision.
- **Task**: one running copy of a task definition. When it stops, its stop reason and exit codes say why.
- **Service**: keeps a desired number of tasks running, replaces failed ones, and rolls out new revisions as deployments.

## In neboto

- **Tasks**: running tasks plus the most recent stopped ones (up to 100 per cluster). ECS keeps stopped tasks for only about an hour, so a crashed task's stop reason is here only briefly. **f** filters All / Running / Stopped.
- **Services**: red when a rollout failed, yellow while one is in progress, **scaled to 0** (dim) when an active service wants no tasks. The pane has Deployments, Tasks, Networking, Events and an Optimizer section.
- On a task, **t** tails its container logs, and **s** starts an ECS Exec shell in it (aws ecs execute-command, opened in a new tmux or terminal window when it can).

## Learn more

- What is Amazon ECS: https://docs.aws.amazon.com/AmazonECS/latest/developerguide/Welcome.html
- Services: https://docs.aws.amazon.com/AmazonECS/latest/developerguide/ecs_services.html
- Task definitions: https://docs.aws.amazon.com/AmazonECS/latest/developerguide/task_definitions.html
- Stopped task errors: https://docs.aws.amazon.com/AmazonECS/latest/developerguide/stopped-task-errors.html
