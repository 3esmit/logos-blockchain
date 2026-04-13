Feature: Zone SDK

  @zone_ci
  Scenario: Publish messages and read them from the zone indexer
    Given I have a zone cluster
    When the zone node is at height 1 in 120 seconds
    And a zone sequencer is initialized
    And a zone indexer is initialized
    And I publish the following zone messages:
      | alias | data           |
      | MSG_1 | Hello, Zone!   |
      | MSG_2 | Second message |
      | MSG_3 | Third message  |
    Then all zone messages are safe in 120 seconds
    And all zone messages are finalized in 180 seconds
    And the zone indexer returns messages in this order:
      | alias |
      | MSG_1 |
      | MSG_2 |
      | MSG_3 |
    When I submit zone set keys transaction "SET_KEYS_1"
    Then zone transaction "SET_KEYS_1" is included in 180 seconds
    And zone transaction "SET_KEYS_1" is finalized in 180 seconds
    And I stop all nodes

  @zone_ci
  Scenario: Resume zone sequencer from checkpoint
    Given I have a zone cluster
    When the zone node is at height 1 in 120 seconds
    And a zone sequencer is initialized
    And a zone indexer is initialized
    And I publish the following zone messages:
      | alias | data      |
      | MSG_1 | Message 1 |
      | MSG_2 | Message 2 |
    And I save the current zone sequencer checkpoint as "CHECKPOINT_1"
    And I restart the zone sequencer from checkpoint "CHECKPOINT_1"
    And I publish the following zone messages:
      | alias | data      |
      | MSG_3 | Message 3 |
      | MSG_4 | Message 4 |
    Then all zone messages are safe in 120 seconds
    And all zone messages are finalized in 180 seconds
    And the zone indexer returns messages in this order:
      | alias |
      | MSG_1 |
      | MSG_2 |
      | MSG_3 |
      | MSG_4 |
    And I stop all nodes

  @zone_ci
  Scenario: Resume zone sequencer from stale checkpoint
    Given I have a zone cluster
    When the zone node is at height 1 in 120 seconds
    And a zone sequencer is initialized
    And a zone indexer is initialized
    And I publish the following zone messages:
      | alias | data  |
      | MSG_1 | msg-1 |
      | MSG_2 | msg-2 |
    And I save the current zone sequencer checkpoint as "STALE_CHECKPOINT"
    Then all zone messages are finalized in 180 seconds
    When I restart the zone sequencer fresh
    And I publish the following zone messages:
      | alias | data  |
      | MSG_3 | msg-3 |
      | MSG_4 | msg-4 |
    Then all zone messages are finalized in 180 seconds
    When I restart the zone sequencer from checkpoint "STALE_CHECKPOINT"
    And I publish the following zone messages:
      | alias | data  |
      | MSG_5 | msg-5 |
    Then all zone messages are finalized in 180 seconds
    And the zone indexer returns each of these messages exactly once in this order:
      | alias |
      | MSG_1 |
      | MSG_2 |
      | MSG_3 |
      | MSG_4 |
      | MSG_5 |
    And I stop all nodes
