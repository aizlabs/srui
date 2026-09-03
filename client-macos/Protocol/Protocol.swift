import Foundation
@_exported import SwiftProtobuf
@_exported import SemanticModel


// Convenient typealiases for SRUI wire types
public typealias SRUINodeRecord = Srui_Protocol_NodeRecord
public typealias SRUITypeRef = Srui_Protocol_TypeRef
public typealias SRUIPropertyRef = Srui_Protocol_PropertyRef
public typealias SRUIProperty = Srui_Protocol_Property
public typealias SRUIValue = Srui_Protocol_Value
public typealias SRUIOperation = Srui_Protocol_Operation
public typealias SRUICommitOp = Srui_Protocol_CommitOp
public typealias SRUITransaction = Srui_Protocol_Transaction
public typealias SRUIEvent = Srui_Protocol_Event
public typealias SRUIStandardEnum = Srui_Protocol_StandardEnum
public typealias SRUIStandardNodeType = Srui_Protocol_StandardNodeType
public typealias SRUIStandardProperty = Srui_Protocol_StandardProperty
public typealias SRUIStandardEvent = Srui_Protocol_StandardEvent
public typealias SRUIStandardOperation = Srui_Protocol_StandardOperation
public typealias SRUIClientHello = Srui_Protocol_ClientHello
public typealias SRUIServerWelcome = Srui_Protocol_ServerWelcome
public typealias SRUIClientResume = Srui_Protocol_ClientResume
public typealias SRUIServerResumeOk = Srui_Protocol_ServerResumeOk
public typealias SRUIServerResyncRequired = Srui_Protocol_ServerResyncRequired
public typealias SRUISessionContinuity = Srui_Protocol_SessionContinuity
public typealias SRUIServerEventAck = Srui_Protocol_ServerEventAck
public typealias SRUIEventAckStatus = Srui_Protocol_EventAckStatus
public typealias SRUIMessage = Srui_Protocol_SruiMessage

public typealias SRUIModelItem = Srui_Protocol_ModelItem
public typealias SRUICreateModelOp = Srui_Protocol_CreateModelOp
public typealias SRUIModelInsertOp = Srui_Protocol_ModelInsertOp
public typealias SRUIModelDeleteOp = Srui_Protocol_ModelDeleteOp
public typealias SRUIModelUpdateOp = Srui_Protocol_ModelUpdateOp
public typealias SRUIModelResetRangeOp = Srui_Protocol_ModelResetRangeOp

public typealias SRUIResourceMetadata = Srui_Protocol_ResourceMetadata
public typealias SRUIResourceChunk = Srui_Protocol_ResourceChunk
public typealias SRUIResourcePriority = Srui_Protocol_ResourcePriority

public struct ProtocolPlaceholder {
    public init() {}
}
