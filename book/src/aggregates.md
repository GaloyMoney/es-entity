# Aggregates

In Domain Driven Design an `aggregate` represents a collection of `entities` that must be atomically updated in order to keep the business rules intanct.
One `entity` is designated the `aggregate root` which is responsible for enforcing that the relationships between the component entities hold.

## Keeping Aggregates Small

In practice it is preferable to keep your `aggregates` as small as possible.
In most cases the `aggregate` should only contain a single `entity` - in which case the `aggregate` is indistinguishable from the single `entity` it contains.

Keeping `aggregates` small by applying careful design and consideration of the domain invariants and their boundaries promotes decoupling and reduces over all complexity.
Its easier to reason about and test the behaviour of "a single thing" vs "a bunch of things" in aggregate.

## When to Use Aggregates

A common misconception is that once you have identified a parent-child relationship you should naturally represent them as an aggregate.
This is however not the case - not every entity relationship that intuitively presents as a parent-child hierarchy needs to be modelled as an aggregate.
Only when there is a business rule that inherently spans the relationship does it become mandatory.

### Example: Subscription and Billing Periods

An example of this could be if you have a `Subscription` that has successive `BillingPeriods`.
Say a use case specifies that the system should be able to add a line item to the current `BillingPeriod`.

The emphesis here is on the word `current` - the domain invariant is that there may only be a single `current` `BillingPeriod` per `Subscription`.
But how do we enforce that?
To keep the system consistent we need a "thing" that tracks all `BillingPeriods` and enforces the uniqueness of the `current` state across them.
A `BillingPeriod` entity is not aware of the other ones and can therefore not enforce whether or not `current` status is indeed unique or not.

## Implementation Approaches

### 1. Simple Foreign Key Relationship

Lets first consider an approach that treats the entities as separate with a simple foreign key relationship:
```rust,ignore
let subscription = subs.find_by_id(id).await?;
let billing_period_id = subscription.current_billing_period_id();

let mut billing_period = periods.find_by_id(billing_period_id).await?;
billing_period.add_line_item(amount);
periods.update(&mut billing_period).await?;
```

The risk here is that it is possible that the period which is the `current` one changed between the line that queries the subscription and the line that updates the period.
It is of course possible to prohibit this given the correct implementation - but that puts the burdon on the developer and may be brittle.
In general the foreign key approach may lead to inconsistent states in edge cases.
Depending on the specifics of the domain and the processes that would need to be invoked if this edge case is hit that may or may not be acceptable.

### 2. Transactional Approach

One way of removing this edge case is using the transactional guarantees of the database to enforce consistency across the 2 entities.
To achieve this you would probably have to use `SERIALIZABLE` isolation level - which adds a lot of overhead to the database.

```rust,ignore
let mut tx = pool.begin().await?;
sqlx::query!("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE").execute(&mut *tx).await?;

let subscription = subs.find_by_id_in_op(&mut tx, id).await?;
let billing_period_id = subscription.current_billing_period_id();

let mut billing_period = billing_periods.find_by_id_in_op(&mut tx, billing_period_id).await?;
billing_period.add_line_item(amount);
billing_periods.update_in_op(&mut tx, &mut billing_period).await?;

tx.commit().await?;
```

By using a database transaction we have essentially created a `logical-aggregate` where we are using the features of the database to enforce consistency.
Thus eleviating some of the cognitive overhead and risk.
We still have to remember however that this `logical-aggregate` exists (even if not visible in the code) and ensure that we always set the correct transaction boundaries and isolation level when we are accessing the `BillingPeriods`.

### 3. Nested Aggregate Approach

To make the relationship more obvious and make it impossible to introduce edge cases we could also to nest the `BillingPeriod` inside the `Subscription`.
This way we use the modeling and relationships of our `struct`s to reflect the `aggregate` relationship more clearly.
All access to the `BillingPeriod` would be moderated by the `Subscription` root.
And updates would be proxied via the root entity to guarantee we are updating the correct one.
```rust,ignore
let mut subscription = subs.find_by_id(id).await?;
subscription.add_line_item_to_current_billing_period(amount);
subs.update(&mut subscription).await?;
```

This essentially removes all edge cases and guarantees atomicity on the type level - which is good.
But it introduces some complexity on handling the nesting itself.

### 4. Domain Restructuring

Finally we could simply re-structure the `entities` so that you do not need any kind of 'higher-level' enforcement.
An example might be to represent `CurrentBillingPeriod` and `ClosedBillingPeriod` as separate entities entirely.
In the real world this approach would probably be better than any of the examples above.
After all, if a "thing" has fundamentally different domain rules when its in 1 state vs another state - why not simply represent the two states as two separate `entities`? Especially if the restructuring allows you to reduce the size of your `aggregates`.

That would make the implementation look something like:
```rust,ignore
let mut billing_period = current_billing_periods.find_by_subscription_id(subscription_id).await?;
billing_period.add_line_item(amount);
current_billing_periods.update(&mut billing_period).await?;
```
The `current_billing_periods` repository could not return a non-current one thereby sidestepping the coordination issue entirely.

## Summary

Of the discussed options 3 of the 4 approaches (simple foreign key, transactional, restructuring) can be handled with the features of `es-entity` that were previously discussed.
The nested approach requires special support from your repository to correctly persist all the nested entities and hydrate them when loading the root.
If taking all options into consideration you decide the correct approach to solving your domain constraint is via nesting the next section will describe how to represent that using `es-entity`.
