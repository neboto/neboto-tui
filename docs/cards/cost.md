# Cost Explorer

Cost Explorer reports what the account (or, from the management account, the organization) spent, grouped and filtered by service, account, region, usage type, tag or cost category. The data trails real usage by up to a day.

## Key ideas

- **Unblended cost**: what each usage was charged at the time, before credits are spread or savings plans amortized.
- **Cost allocation tags**: tags only show up in cost data after they are activated in Billing, and only from then on.
- **Cost categories**: your own rules that map spend into named buckets (teams, products).
- **Anomalies**: Cost Anomaly Detection flags spend that departs from the usual pattern, once a monitor is set up.

## In neboto

- **1**–**6** group by service, linked account, region, usage type, tag or cost category; **7** shows anomalies. **[** and **]** step the period (month to date, last month, last 3 months) and **t** cycles it.
- Opening a row fetches its Breakdown, Regions, Trend and Forecast. The forecast is always this month's.
- **Every Cost Explorer request is billed** (about $0.01 each), so results are cached for 6 hours and the tag and category pickers load only when you first open them.
- The figures are unblended spend.

## Learn more

- What is Cost Explorer: https://docs.aws.amazon.com/cost-management/latest/userguide/ce-what-is.html
- Cost allocation tags: https://docs.aws.amazon.com/awsaccountbilling/latest/aboutv2/cost-alloc-tags.html
- Cost categories: https://docs.aws.amazon.com/awsaccountbilling/latest/aboutv2/manage-cost-categories.html
- Cost Anomaly Detection: https://docs.aws.amazon.com/cost-management/latest/userguide/manage-ad.html
